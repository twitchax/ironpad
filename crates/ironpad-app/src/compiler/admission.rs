//! Build admission control (PRD-0052): a global cap on concurrent cargo
//! processes plus a per-client price on *starting* one.
//!
//! The scarce resource is a cargo process, not an HTTP request. Cache hits
//! never pass through here; [`crate::server_fns`] consults admission only
//! after a confirmed cache miss, so warmed notebooks (and the Playwright
//! suite) stay free while N distinct cell ids can no longer buy N concurrent
//! 300-second builds on one machine.
//!
//! Two admission modes, matched to their caller's patience:
//! - **Compiles** wait for a slot, bounded by a queue timeout; exhaustion is
//!   a clear "at capacity" error rather than a socket held open forever.
//! - **Live checks** `try_acquire` and shed load: the client already treats
//!   `Skipped` as "try again after the next quiet period" (PRD-0045), so
//!   typing never blocks on capacity.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tracing::Instrument as _;

/// Default cap on concurrent cargo processes (compiles and checks pool
/// separately, so a burst of checks cannot starve builds or vice versa).
pub const DEFAULT_MAX_CONCURRENT_BUILDS: usize = 3;

/// Default per-client token bucket: `BURST` builds instantly, refilling at
/// `PER_MIN` per minute. Generous on purpose — it exists to stop scripted
/// abuse, not a human iterating on a cell. Sized with the error loop in
/// mind: FAILED compiles are never cached, so someone debugging a cell pays
/// a token per attempt (one per 2s sustained is faster than any human
/// edit-compile loop; the Playwright suite tripped a 10/min version of this
/// and overrides the env instead).
const DEFAULT_RATE_BURST: f64 = 20.0;

/// Per-client token bucket for `get_browserpod_key` (PRD-0066).
///
/// Deliberately its OWN bucket rather than a share of the build budget. The
/// two are unrelated costs: a key fetch is one string, a build is a cargo
/// process, and charging pod boots against build tokens would let a notebook
/// full of Linux cells rate-limit its own compiles.
///
/// Sized so legitimate use never notices. One fetch happens per pod boot, one
/// pod serves a whole notebook, so a reader hitting this is reloading dozens
/// of times a minute. A scraper collecting the key in bulk is what it bounds:
/// the prod key is origin-locked, so this is depth rather than the only lock.
const DEFAULT_KEY_RATE_BURST: f64 = 30.0;

/// Refill rate for [`DEFAULT_KEY_RATE_BURST`], per minute.
const DEFAULT_KEY_RATE_PER_MIN: f64 = 30.0;
const DEFAULT_RATE_PER_MIN: f64 = 30.0;

/// Default bound on how long a compile may queue for a slot.
const DEFAULT_QUEUE_TIMEOUT_SECS: u64 = 180;

/// Buckets at or above this count trigger an opportunistic sweep of
/// fully-recharged (i.e. idle) entries, so the map cannot grow without bound
/// across many distinct client IPs.
const BUCKET_SWEEP_THRESHOLD: usize = 1024;

/// Why a compile was refused admission.
#[derive(Debug, PartialEq, Eq)]
pub enum AdmissionDenied {
    /// The per-client bucket is empty: this client started too many builds
    /// too fast.
    RateLimited,
    /// Every build slot stayed occupied for the whole queue timeout.
    AtCapacity,
}

impl std::fmt::Display for AdmissionDenied {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RateLimited => write!(
                f,
                "rate limited: too many builds started from this client. Cached runs are unaffected; wait a minute and retry"
            ),
            Self::AtCapacity => write!(
                f,
                "the server is at build capacity right now. Cached runs are unaffected; retry shortly"
            ),
        }
    }
}

/// Per-client token bucket state.
struct Bucket {
    tokens: f64,
    last_refill: Instant,
}

/// Shared admission state. Cloneable; all clones share the same slots and
/// buckets (mirrors [`super::CompileLocks`]).
#[derive(Clone)]
pub struct BuildAdmission {
    compile_slots: Arc<tokio::sync::Semaphore>,
    check_slots: Arc<tokio::sync::Semaphore>,
    buckets: Arc<Mutex<HashMap<String, Bucket>>>,
    key_buckets: Arc<Mutex<HashMap<String, Bucket>>>,
    rate_burst: f64,
    rate_per_sec: f64,
    key_rate_burst: f64,
    key_rate_per_sec: f64,
    queue_timeout: Duration,
}

impl Default for BuildAdmission {
    /// Production construction: the concurrency cap comes from the server's
    /// `--max-concurrent-builds`, everything else from env overrides that
    /// follow the `IRONPAD_BUILD_TIMEOUT_SECS` precedent.
    fn default() -> Self {
        Self::from_env(DEFAULT_MAX_CONCURRENT_BUILDS)
    }
}

impl BuildAdmission {
    /// Build with `max_concurrent` slots and rate/queue settings from the
    /// environment (`IRONPAD_BUILD_RATE_BURST`, `IRONPAD_BUILD_RATE_PER_MIN`,
    /// `IRONPAD_BUILD_QUEUE_TIMEOUT_SECS`), falling back to the defaults.
    pub fn from_env(max_concurrent: usize) -> Self {
        let env_f64 = |name: &str, default: f64| {
            std::env::var(name)
                .ok()
                .and_then(|v| v.parse::<f64>().ok())
                .filter(|v| *v > 0.0)
                .unwrap_or(default)
        };
        let queue_timeout_secs = std::env::var("IRONPAD_BUILD_QUEUE_TIMEOUT_SECS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(DEFAULT_QUEUE_TIMEOUT_SECS);

        Self::new(
            max_concurrent,
            env_f64("IRONPAD_BUILD_RATE_BURST", DEFAULT_RATE_BURST),
            env_f64("IRONPAD_BUILD_RATE_PER_MIN", DEFAULT_RATE_PER_MIN),
            Duration::from_secs(queue_timeout_secs),
        )
        .with_key_rate(
            env_f64("IRONPAD_KEY_RATE_BURST", DEFAULT_KEY_RATE_BURST),
            env_f64("IRONPAD_KEY_RATE_PER_MIN", DEFAULT_KEY_RATE_PER_MIN),
        )
    }

    /// Fully-explicit construction (tests).
    pub fn new(
        max_concurrent: usize,
        rate_burst: f64,
        rate_per_min: f64,
        queue_timeout: Duration,
    ) -> Self {
        let slots = max_concurrent.max(1);
        Self {
            compile_slots: Arc::new(tokio::sync::Semaphore::new(slots)),
            check_slots: Arc::new(tokio::sync::Semaphore::new(slots)),
            buckets: Arc::new(Mutex::new(HashMap::new())),
            key_buckets: Arc::new(Mutex::new(HashMap::new())),
            rate_burst,
            rate_per_sec: rate_per_min / 60.0,
            key_rate_burst: DEFAULT_KEY_RATE_BURST,
            key_rate_per_sec: DEFAULT_KEY_RATE_PER_MIN / 60.0,
            queue_timeout,
        }
    }

    /// Override the `get_browserpod_key` bucket (tests, and `from_env`).
    #[must_use]
    pub fn with_key_rate(mut self, burst: f64, per_min: f64) -> Self {
        self.key_rate_burst = burst;
        self.key_rate_per_sec = per_min / 60.0;
        self
    }

    /// Admit a compile that is about to spawn cargo: charge the client's
    /// bucket, then wait (bounded) for a build slot. The returned permit must
    /// be held for the whole build.
    ///
    /// The bucket is charged before the wait — a client at its rate limit
    /// should hear so immediately, not after queueing.
    pub async fn admit_compile(
        &self,
        client_ip: &str,
    ) -> Result<tokio::sync::OwnedSemaphorePermit, AdmissionDenied> {
        if !self.try_take_token(client_ip) {
            tracing::warn!(client_ip, "build rate limit hit");
            return Err(AdmissionDenied::RateLimited);
        }

        let acquire = Arc::clone(&self.compile_slots).acquire_owned();
        match tokio::time::timeout(self.queue_timeout, acquire)
            .instrument(tracing::info_span!("build_permit_wait"))
            .await
        {
            Ok(Ok(permit)) => Ok(permit),
            // Closed can't happen (nothing closes the semaphore), but map it
            // to the same user-visible refusal rather than panicking.
            Ok(Err(_)) => Err(AdmissionDenied::AtCapacity),
            Err(_) => {
                tracing::warn!(client_ip, "build queue timeout — at capacity");
                Err(AdmissionDenied::AtCapacity)
            }
        }
    }

    /// Admit a live check, or `None` when every check slot is busy — the
    /// caller degrades to `CheckStatus::Skipped`. Checks are not rate
    /// limited: they are cheap, self-bounded by the live-check timeout, and
    /// fire on-type.
    pub fn try_admit_check(&self) -> Option<tokio::sync::OwnedSemaphorePermit> {
        Arc::clone(&self.check_slots).try_acquire_owned().ok()
    }

    /// Admit one `get_browserpod_key` request. `true` = admitted.
    ///
    /// Charges the key bucket, which is separate from the build bucket: see
    /// [`DEFAULT_KEY_RATE_BURST`] for why the two budgets do not mix. Refusal
    /// is not an error the reader can act on, so the caller reports the same
    /// "no key configured" shape a contributor checkout produces rather than
    /// inventing a second failure mode in the cell UI.
    pub fn try_admit_key(&self, client_ip: &str) -> bool {
        let admitted = take_token(
            &self.key_buckets,
            client_ip,
            self.key_rate_burst,
            self.key_rate_per_sec,
        );
        if !admitted {
            tracing::warn!(client_ip, "browserpod key rate limit hit");
        }
        admitted
    }

    /// Take one token from `client_ip`'s build bucket, refilling for elapsed
    /// time first. `true` = admitted.
    fn try_take_token(&self, client_ip: &str) -> bool {
        take_token(&self.buckets, client_ip, self.rate_burst, self.rate_per_sec)
    }
}

/// Take one token from `client_ip`'s bucket in `table`, refilling for elapsed
/// time first. `true` = admitted.
///
/// A free function over the table rather than a method, so the build bucket
/// and the key bucket share one implementation of the refill, the sweep and
/// the off-by-one that decides admission. Two copies of this drift.
fn take_token(
    table: &Mutex<HashMap<String, Bucket>>,
    client_ip: &str,
    burst: f64,
    per_sec: f64,
) -> bool {
    let now = Instant::now();
    let mut buckets = table.lock().expect("bucket table poisoned");

    // Opportunistic sweep: a bucket back at full charge has been idle for
    // at least burst/rate seconds and carries no information.
    if buckets.len() >= BUCKET_SWEEP_THRESHOLD {
        buckets.retain(|_, b| {
            let refilled = b.tokens + now.duration_since(b.last_refill).as_secs_f64() * per_sec;
            refilled < burst
        });
    }

    let bucket = buckets.entry(client_ip.to_string()).or_insert(Bucket {
        tokens: burst,
        last_refill: now,
    });
    bucket.tokens =
        (bucket.tokens + now.duration_since(bucket.last_refill).as_secs_f64() * per_sec).min(burst);
    bucket.last_refill = now;

    if bucket.tokens >= 1.0 {
        bucket.tokens -= 1.0;
        true
    } else {
        false
    }
}

/// Client identity for the rate limiter, from proxy headers.
///
/// `Fly-Client-IP` is set by Fly's edge and wins; `X-Forwarded-For` (first
/// hop) covers other reverse proxies. Bare local traffic shares one "local"
/// bucket, which is fine: dev boxes and the e2e suite compile against a warm
/// cache, and admission is only consulted on a miss.
pub fn client_ip(headers: &http::HeaderMap) -> String {
    headers
        .get("fly-client-ip")
        .and_then(|v| v.to_str().ok())
        .or_else(|| {
            headers
                .get("x-forwarded-for")
                .and_then(|v| v.to_str().ok())
                .map(|v| v.split(',').next().unwrap_or("").trim())
        })
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("local")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn admission(max: usize, burst: f64, per_min: f64, queue_ms: u64) -> BuildAdmission {
        BuildAdmission::new(max, burst, per_min, Duration::from_millis(queue_ms))
    }

    #[tokio::test]
    async fn burst_is_admitted_then_rate_limited() {
        let a = admission(4, 2.0, 60.0, 100);
        assert!(a.admit_compile("1.2.3.4").await.is_ok());
        assert!(a.admit_compile("1.2.3.4").await.is_ok());
        assert_eq!(
            a.admit_compile("1.2.3.4").await.unwrap_err(),
            AdmissionDenied::RateLimited
        );
        // A different client has its own bucket.
        assert!(a.admit_compile("5.6.7.8").await.is_ok());
    }

    #[tokio::test]
    async fn bucket_refills_over_time() {
        // 1 token burst, refilling at 600/min = 10/s → ~100ms per token.
        let a = admission(8, 1.0, 600.0, 100);
        assert!(a.admit_compile("ip").await.is_ok());
        assert_eq!(
            a.admit_compile("ip").await.unwrap_err(),
            AdmissionDenied::RateLimited
        );
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert!(
            a.admit_compile("ip").await.is_ok(),
            "elapsed time must refill the bucket"
        );
    }

    #[tokio::test]
    async fn all_slots_held_times_out_as_at_capacity() {
        let a = admission(1, 100.0, 6000.0, 50);
        let _held = a.admit_compile("a").await.unwrap();
        assert_eq!(
            a.admit_compile("b").await.unwrap_err(),
            AdmissionDenied::AtCapacity
        );
    }

    #[tokio::test]
    async fn a_released_slot_admits_the_next_waiter() {
        let a = admission(1, 100.0, 6000.0, 5000);
        let held = a.admit_compile("a").await.unwrap();
        let a2 = a.clone();
        let waiter = tokio::spawn(async move { a2.admit_compile("b").await });
        tokio::time::sleep(Duration::from_millis(20)).await;
        drop(held);
        assert!(waiter.await.unwrap().is_ok(), "queued compile must proceed");
    }

    #[test]
    fn checks_shed_load_instead_of_queueing() {
        let a = admission(2, 100.0, 6000.0, 50);
        let p1 = a.try_admit_check().unwrap();
        let _p2 = a.try_admit_check().unwrap();
        assert!(
            a.try_admit_check().is_none(),
            "an exhausted pool must skip, not wait"
        );
        drop(p1);
        assert!(a.try_admit_check().is_some(), "released slot is reusable");
    }

    #[test]
    fn key_requests_are_rate_limited_per_client() {
        let a = admission(4, 20.0, 20.0, 50).with_key_rate(3.0, 60.0);

        for i in 0..3 {
            assert!(
                a.try_admit_key("1.2.3.4"),
                "burst request {i} must be admitted"
            );
        }
        assert!(
            !a.try_admit_key("1.2.3.4"),
            "past the burst must be refused"
        );

        // Per client, not global: one scraper must not lock every reader out.
        assert!(
            a.try_admit_key("5.6.7.8"),
            "a different client has its own bucket"
        );
    }

    /// The key bucket and the build bucket must not share a budget.
    ///
    /// A notebook full of Linux cells boots pods and fetches the key; if that
    /// spent build tokens, running the notebook would rate-limit its own
    /// compiles. The negative control is the point here: exhausting one
    /// budget entirely must leave the other untouched.
    #[tokio::test]
    async fn key_and_build_budgets_are_independent() {
        let a = admission(4, 2.0, 1.0, 50).with_key_rate(2.0, 1.0);

        assert!(a.try_admit_key("1.2.3.4"));
        assert!(a.try_admit_key("1.2.3.4"));
        assert!(!a.try_admit_key("1.2.3.4"), "key budget is now spent");

        // Builds are untouched by the spent key budget.
        assert!(a.admit_compile("1.2.3.4").await.is_ok());
        assert!(a.admit_compile("1.2.3.4").await.is_ok());
        assert!(
            matches!(
                a.admit_compile("1.2.3.4").await,
                Err(AdmissionDenied::RateLimited)
            ),
            "build budget is separate and spends on its own schedule"
        );
    }

    #[test]
    fn client_ip_prefers_fly_then_xff_then_local() {
        let mut h = http::HeaderMap::new();
        assert_eq!(client_ip(&h), "local");

        h.insert("x-forwarded-for", "9.9.9.9, 10.0.0.1".parse().unwrap());
        assert_eq!(client_ip(&h), "9.9.9.9");

        h.insert("fly-client-ip", "1.2.3.4".parse().unwrap());
        assert_eq!(client_ip(&h), "1.2.3.4", "the edge header must win");
    }
}
