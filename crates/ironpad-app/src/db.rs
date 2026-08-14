//! Accounts database (PRD-0053) — embedded `SurrealDB` on the data mount.
//!
//! One `SurrealKV` file holds users, sessions, mutable shares, and RBAC grants.
//! The wasm/js blobs stay on disk in the content-addressed store; only
//! pointers and notebook JSON live here, so share content and ownership
//! update transactionally.
//!
//! Session tokens are stored hashed (blake3 of the cookie value is the record
//! key), matching the project's at-rest posture: a leaked DB file yields no
//! live cookies.

use std::path::Path;

use anyhow::{Context as _, Result};
use surrealdb::engine::local::{Db as LocalDb, SurrealKv};
use surrealdb::types::SurrealValue;
use surrealdb::Surreal;

// ── Constants ───────────────────────────────────────────────────────────────

/// Sliding session lifetime.
pub(crate) const SESSION_TTL_SECS: i64 = 30 * 24 * 60 * 60;

/// Renew a session at most this often: `session_user` bumps `expires_at` only
/// once the session has aged past this, so validation is not a write per
/// request.
const SESSION_RENEW_AFTER_SECS: i64 = 12 * 60 * 60;

/// The one resource kind minted today. EDIT/READ and other kinds are data
/// changes on the same table (PRD-0053 principles).
const KIND_MUTABLE_SHARE: &str = "mutable_share";

/// The creator's role: full control (push, discard, privacy, grants, delete).
const ROLE_OWNER: &str = "OWNER";

/// Read access to a PRIVATE share (PRD-0061). Public shares need no grant.
const ROLE_READ: &str = "READ";

// ── Public types ────────────────────────────────────────────────────────────

/// A signed-in user, as resolved from a session token.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, SurrealValue)]
pub struct AuthUser {
    /// GitHub's numeric user id, as a decimal string (it is the `user` record
    /// key and the stable identity; logins can be renamed on GitHub).
    pub github_id: String,
    pub login: String,
    pub avatar_url: String,
}

/// One user as the admin panel shows them (PRD-0063 T-005).
#[derive(Debug, Clone)]
pub struct AdminUserRow {
    pub github_id: String,
    pub login: String,
    pub avatar_url: String,
    pub created_at: String,
    pub sessions: u64,
    pub owned_shares: u64,
}

/// One mutable share row, with its owner attribution resolved.
///
/// `notebook_json` is the PUBLISHED copy: `None` for an account notebook that
/// has never been published (PRD-0064), whose content lives in the draft slot
/// and is reachable only through the owner's editing view
/// ([`get_share_for_edit`](Db::get_share_for_edit)). Reader-facing callers
/// treat `None` as "no such published notebook".
#[derive(Debug, Clone)]
pub struct MutableShareRow {
    pub notebook_json: Option<String>,
    pub manifest_json: Option<String>,
    /// Only the owner and READ grantees may view (PRD-0061).
    pub private: bool,
    /// `None` only for a share whose grant row is missing (should not happen;
    /// creates are transactional).
    pub owner: Option<AuthUser>,
}

/// Summary row for the owner's account listing: every share they own,
/// published or not (PRD-0064).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct OwnedShareRow {
    pub id: String,
    /// Draft-or-published content, so the listing titles an unpublished
    /// notebook from the copy that actually exists.
    pub notebook_json: String,
    /// `None` until the first publish.
    pub pushed_at: Option<String>,
    pub created_at: String,
    /// Whether a published copy exists (the reader-visible one).
    pub published: bool,
}

impl OwnedShareRow {
    /// See [`ironpad_common::last_activity`]: this is that one rule, applied
    /// to this struct's fields. It holds no rule of its own.
    ///
    /// The db sort and the home listing read the same timestamp through this
    /// helper, so they cannot disagree about which notebook is most recent.
    #[must_use]
    pub fn last_activity(&self) -> &str {
        ironpad_common::last_activity(self.pushed_at.as_deref(), &self.created_at)
    }
}

/// The owner's editing view of a share (PRD-0054): the draft when one
/// exists, else the published copy, plus whether they differ.
#[derive(Debug, Clone)]
pub struct ShareEditRow {
    /// Draft content if a draft exists, otherwise the published content.
    pub notebook_json: String,
    /// True when a draft exists (draft may differ from published). An
    /// unpublished account notebook is permanently dirty by construction:
    /// its content IS the draft (PRD-0064).
    pub dirty: bool,
    /// Whether a published copy exists. `dirty` alone cannot tell an
    /// unpublished notebook from a published one with pending edits, and
    /// the editor's button says Publish for the first and Push for the
    /// second.
    pub published: bool,
    /// The share's privacy flag, for the owner's Access UI (PRD-0061).
    pub private: bool,
}

// ── Handle ──────────────────────────────────────────────────────────────────

/// Cloneable handle to the embedded database. Provided as leptos context (for
/// `#[server]` fns) and carried in the Axum state (for auth/OG handlers).
#[derive(Clone)]
pub struct Db {
    inner: Surreal<LocalDb>,
}

/// Is this a retryable optimistic-concurrency conflict?
///
/// Matched on the message because the engine's error arrives wrapped in
/// `anyhow` context by the time callers see it, and `SurrealDB` reports the
/// conflict as a generic query error rather than a distinct variant. The
/// engine writes the retry advice into the text itself ("This transaction
/// can be retried"), so the string IS the contract here.
fn is_write_conflict(e: &anyhow::Error) -> bool {
    let msg = format!("{e:#}");
    msg.contains("Transaction conflict") || msg.contains("can be retried")
}

/// Run a write, retrying while the engine reports a retryable conflict.
///
/// `SurrealKV` is optimistically concurrent: two writers touching ONE record
/// race, one commits, and the losers get a conflict. Nothing retried it.
/// Measured before this existed: 7 of 8 parallel `save_draft` calls against
/// a single share failed, which is two browser tabs autosaving one account
/// notebook (PRD-0064 made server drafts the primary storage path), and 7 of
/// 8 concurrent sign-ins as one user returned 500 over HTTP.
///
/// Bounded rather than unbounded: a conflict that survives five attempts is
/// not contention any more, and burying it would turn a fast error into a
/// hung request. Backoff carries jitter because retries that resynchronise
/// collide again on the next attempt.
async fn with_conflict_retry<F, Fut, T>(op: F) -> Result<T>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = Result<T>>,
{
    const ATTEMPTS: u32 = 5;
    for attempt in 1..=ATTEMPTS {
        match op().await {
            Err(e) if attempt < ATTEMPTS && is_write_conflict(&e) => {
                let jitter = u64::from(
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.subsec_nanos() % 4)
                        .unwrap_or(0),
                );
                let backoff = (1u64 << (attempt - 1)) + jitter;
                tracing::debug!(attempt, backoff_ms = backoff, "write conflict; retrying");
                tokio::time::sleep(std::time::Duration::from_millis(backoff)).await;
            }
            other => return other,
        }
    }
    unreachable!("loop returns on the final attempt")
}

impl Db {
    /// Open (creating if needed) the database file and apply the schema.
    #[tracing::instrument(name = "db_open", level = "info", skip_all)]
    pub async fn open(path: &Path) -> Result<Self> {
        let inner = Surreal::new::<SurrealKv>(path)
            .await
            .with_context(|| format!("failed to open database at {}", path.display()))?;
        inner
            .use_ns("ironpad")
            .use_db("ironpad")
            .await
            .context("failed to select namespace")?;

        let db = Self { inner };
        db.define_schema().await?;
        db.sweep_expired_sessions().await?;
        Ok(db)
    }

    /// Idempotent schema definition, run on every boot.
    async fn define_schema(&self) -> Result<()> {
        self.inner
            .query(
                "
                DEFINE TABLE IF NOT EXISTS meta SCHEMAFULL;
                DEFINE FIELD IF NOT EXISTS value ON meta TYPE string;

                DEFINE TABLE IF NOT EXISTS user SCHEMAFULL;
                DEFINE FIELD IF NOT EXISTS login ON user TYPE string;
                DEFINE FIELD IF NOT EXISTS avatar_url ON user TYPE string;
                DEFINE FIELD IF NOT EXISTS created_at ON user TYPE string;

                DEFINE TABLE IF NOT EXISTS session SCHEMAFULL;
                DEFINE FIELD IF NOT EXISTS user ON session TYPE record<user>;
                DEFINE FIELD IF NOT EXISTS expires_at ON session TYPE int;

                DEFINE TABLE IF NOT EXISTS mutable_share SCHEMAFULL;
                -- OVERWRITE, deliberately, on exactly these two (PRD-0064).
                -- An account notebook has no published copy, so both widen to
                -- option<string>. IF NOT EXISTS silently declines to redefine
                -- an existing field, and CI opens a fresh database every run,
                -- so the widening would look applied everywhere except the one
                -- database that predates it. Every other field is unchanged
                -- and keeps IF NOT EXISTS.
                DEFINE FIELD OVERWRITE notebook_json ON mutable_share TYPE option<string>;
                DEFINE FIELD IF NOT EXISTS manifest_json ON mutable_share TYPE option<string>;
                DEFINE FIELD IF NOT EXISTS draft_json ON mutable_share TYPE option<string>;
                DEFINE FIELD IF NOT EXISTS draft_bytes ON mutable_share TYPE option<int>;
                DEFINE FIELD IF NOT EXISTS private ON mutable_share TYPE bool DEFAULT false;
                DEFINE FIELD IF NOT EXISTS bytes ON mutable_share TYPE int;
                DEFINE FIELD OVERWRITE pushed_at ON mutable_share TYPE option<string>;
                DEFINE FIELD IF NOT EXISTS created_at ON mutable_share TYPE string;

                DEFINE TABLE IF NOT EXISTS rbac_grant SCHEMAFULL;
                DEFINE FIELD IF NOT EXISTS user ON rbac_grant TYPE record<user>;
                DEFINE FIELD IF NOT EXISTS resource_kind ON rbac_grant TYPE string;
                DEFINE FIELD IF NOT EXISTS resource_id ON rbac_grant TYPE string;
                DEFINE FIELD IF NOT EXISTS role ON rbac_grant TYPE string;
                DEFINE INDEX IF NOT EXISTS grant_unique ON rbac_grant \
                    FIELDS user, resource_kind, resource_id, role UNIQUE;
                DEFINE INDEX IF NOT EXISTS grant_by_resource ON rbac_grant \
                    FIELDS resource_kind, resource_id;
                DEFINE INDEX IF NOT EXISTS grant_by_user ON rbac_grant FIELDS user;
                ",
            )
            .await
            .context("schema definition failed")?
            .check()
            .context("schema definition returned an error")?;
        Ok(())
    }

    /// Drop sessions that expired before now. Lazy per-access deletion covers
    /// active tokens; this boot-time sweep bounds the table.
    async fn sweep_expired_sessions(&self) -> Result<()> {
        self.inner
            .query("DELETE session WHERE expires_at < $now")
            .bind(("now", now_secs()))
            .await
            .context("session sweep failed")?
            .check()
            .context("session sweep returned an error")?;
        Ok(())
    }

    // ── Users ───────────────────────────────────────────────────────────

    /// Create or refresh a user row from a GitHub identity. `created_at` is
    /// preserved on refresh.
    #[tracing::instrument(name = "db_upsert_user", level = "info", skip_all, fields(github_id = %github_id))]
    pub async fn upsert_user(&self, github_id: &str, login: &str, avatar_url: &str) -> Result<()> {
        with_conflict_retry(|| async {
            self.inner
                .query(
                    "UPSERT type::record('user', $id) SET \
                        login = $login, \
                        avatar_url = $avatar_url, \
                        created_at = created_at ?? $now",
                )
                .bind(("id", github_id.to_string()))
                .bind(("login", login.to_string()))
                .bind(("avatar_url", avatar_url.to_string()))
                .bind(("now", now_rfc3339()))
                .await
                .context("user upsert failed")?
                .check()
                .context("user upsert returned an error")?;
            Ok(())
        })
        .await
    }

    /// The `github_id` pinned as this instance's administrator, if one has
    /// been recorded (PRD-0063 T-002).
    pub async fn admin_pin(&self) -> Result<Option<String>> {
        let mut res = self
            .inner
            .query("SELECT VALUE value FROM meta:admin_github_id")
            .await
            .context("admin pin read failed")?;
        res.take(0).context("admin pin read returned an error")
    }

    /// Record `github_id` as the pinned administrator, if nothing is pinned.
    ///
    /// Trust on first use: the configured value is a readable GitHub login,
    /// but a login is not a stable identity. GitHub frees a renamed handle for
    /// anyone to claim, so matching the login alone would transfer admin to
    /// whoever claimed it. Pinning the numeric id on the first successful
    /// match means a rename fails closed instead.
    ///
    /// Returns the pin in force after the call, which is the existing one when
    /// there already was one.
    pub async fn pin_admin(&self, github_id: &str) -> Result<String> {
        with_conflict_retry(|| async {
            if let Some(existing) = self.admin_pin().await? {
                return Ok(existing);
            }
            self.inner
                .query("UPSERT meta:admin_github_id SET value = $id")
                .bind(("id", github_id.to_string()))
                .await
                .context("admin pin write failed")?
                .check()
                .context("admin pin write returned an error")?;
            Ok(github_id.to_string())
        })
        .await
    }

    /// Row counts for the admin overview (PRD-0063).
    ///
    /// One query per table rather than a join: they are unrelated tables and
    /// the panel wants a number from each, so a join would only make the
    /// failure modes harder to read.
    pub async fn instance_counts(&self) -> Result<(u64, u64, u64)> {
        /// `GROUP ALL` yields one row shaped `{ count: N }`, not a bare
        /// number: `SELECT VALUE count()` deserialises as an object and fails
        /// with "Expected number, got object".
        #[derive(SurrealValue)]
        struct CountRow {
            count: u64,
        }

        async fn count(db: &Surreal<LocalDb>, table: &str) -> Result<u64> {
            let sql = format!("SELECT count() FROM {table} GROUP ALL");
            let mut res = db
                .query(sql)
                .await
                .with_context(|| format!("count of {table} failed"))?;
            let row: Option<CountRow> = res
                .take(0)
                .with_context(|| format!("count of {table} returned an error"))?;
            // An empty table yields no group row at all, which is 0 rather
            // than a failure.
            Ok(row.map_or(0, |r| r.count))
        }

        Ok((
            count(&self.inner, "user").await?,
            count(&self.inner, "session").await?,
            count(&self.inner, "mutable_share").await?,
        ))
    }

    /// Every user, with the counts the admin panel shows (PRD-0063 T-005).
    ///
    /// Aggregates are grouped queries rather than a count per user: the panel
    /// is a request handler, and N+1 queries against a growing user table is a
    /// shape that only misbehaves once it matters.
    pub async fn list_users_for_admin(&self) -> Result<Vec<AdminUserRow>> {
        #[derive(SurrealValue)]
        struct UserRow {
            github_id: String,
            login: String,
            avatar_url: String,
            created_at: String,
        }
        #[derive(SurrealValue)]
        struct GroupRow {
            user: String,
            count: u64,
        }

        let mut res = self
            .inner
            .query(
                "SELECT record::id(id) AS github_id, login, avatar_url, created_at \
                 FROM user ORDER BY created_at",
            )
            .await
            .context("user list failed")?;
        let users: Vec<UserRow> = res.take(0).context("user list rows malformed")?;

        let mut res = self
            .inner
            .query(
                "SELECT record::id(user) AS user, count() FROM session \
                 GROUP BY user",
            )
            .await
            .context("session counts failed")?;
        let sessions: Vec<GroupRow> = res.take(0).context("session count rows malformed")?;

        let mut res = self
            .inner
            .query(
                "SELECT record::id(user) AS user, count() FROM rbac_grant \
                 WHERE resource_kind = $kind AND role = $role GROUP BY user",
            )
            .bind(("kind", KIND_MUTABLE_SHARE))
            .bind(("role", ROLE_OWNER))
            .await
            .context("share counts failed")?;
        let shares: Vec<GroupRow> = res.take(0).context("share count rows malformed")?;

        let lookup = |rows: &[GroupRow], id: &str| -> u64 {
            rows.iter().find(|r| r.user == id).map_or(0, |r| r.count)
        };

        Ok(users
            .into_iter()
            .map(|u| AdminUserRow {
                sessions: lookup(&sessions, &u.github_id),
                owned_shares: lookup(&shares, &u.github_id),
                github_id: u.github_id,
                login: u.login,
                avatar_url: u.avatar_url,
                created_at: u.created_at,
            })
            .collect())
    }

    /// Delete every session belonging to a user, returning how many went.
    ///
    /// Signs them out everywhere on their next request. Deliberately not a
    /// user deletion: their notebooks and grants are untouched, and they can
    /// sign back in.
    pub async fn revoke_user_sessions(&self, github_id: &str) -> Result<u64> {
        with_conflict_retry(|| async {
            #[derive(SurrealValue)]
            struct Deleted {
                #[allow(dead_code)]
                expires_at: i64,
            }
            let mut res = self
                .inner
                .query("DELETE session WHERE user = type::record('user', $id) RETURN BEFORE")
                .bind(("id", github_id.to_string()))
                .await
                .context("session revoke failed")?;
            let gone: Vec<Deleted> = res.take(0).context("session revoke rows malformed")?;
            Ok(gone.len() as u64)
        })
        .await
    }

    // ── Sessions ────────────────────────────────────────────────────────

    /// Mint a session for a user, returning the plaintext token destined for
    /// the cookie. Only its hash is stored.
    #[tracing::instrument(name = "db_create_session", level = "info", skip_all)]
    pub async fn create_session(&self, github_id: &str) -> Result<String> {
        with_conflict_retry(|| async {
            let token = random_token();
            self.inner
                .query(
                    "CREATE type::record('session', $key) SET \
                        user = type::record('user', $uid), \
                        expires_at = $exp",
                )
                .bind(("key", hash_token(&token)))
                .bind(("uid", github_id.to_string()))
                .bind(("exp", now_secs() + SESSION_TTL_SECS))
                .await
                .context("session create failed")?
                .check()
                .context("session create returned an error")?;
            Ok(token)
        })
        .await
    }

    /// Resolve a session token to its user, or `None` for unknown/expired
    /// tokens. Slides the expiry (at most once per
    /// [`SESSION_RENEW_AFTER_SECS`]), and reports whether it did: a renewal
    /// means the session COOKIE must be re-issued with a fresh `Max-Age`
    /// too, or the browser deletes it 30 days after login while the DB row
    /// slides forever — the "sliding" expiry never actually slid for the
    /// user (the caller side is `crate::auth::current_user`).
    #[tracing::instrument(name = "db_session_user", level = "debug", skip_all)]
    pub async fn session_user(&self, token: &str) -> Result<Option<(AuthUser, bool)>> {
        #[derive(SurrealValue)]
        struct Row {
            github_id: String,
            login: String,
            avatar_url: String,
            expires_at: i64,
        }

        let key = hash_token(token);
        let mut response = self
            .inner
            .query(
                "SELECT record::id(user) AS github_id, user.login AS login, \
                    user.avatar_url AS avatar_url, expires_at \
                 FROM ONLY type::record('session', $key)",
            )
            .bind(("key", key.clone()))
            .await
            .context("session lookup failed")?;
        let Some(row) = response
            .take::<Option<Row>>(0)
            .context("session row malformed")?
        else {
            return Ok(None);
        };

        let now = now_secs();
        if row.expires_at <= now {
            let _ = self
                .inner
                .query("DELETE type::record('session', $key)")
                .bind(("key", key))
                .await;
            return Ok(None);
        }

        let renewed = row.expires_at - now < SESSION_TTL_SECS - SESSION_RENEW_AFTER_SECS;
        if renewed {
            self.inner
                .query("UPDATE type::record('session', $key) SET expires_at = $exp")
                .bind(("key", key))
                .bind(("exp", now + SESSION_TTL_SECS))
                .await
                .context("session renewal failed")?
                .check()
                .context("session renewal returned an error")?;
        }

        Ok(Some((
            AuthUser {
                github_id: row.github_id,
                login: row.login,
                avatar_url: row.avatar_url,
            },
            renewed,
        )))
    }

    /// Delete a session (logout). Unknown tokens are a no-op.
    #[tracing::instrument(name = "db_delete_session", level = "info", skip_all)]
    pub async fn delete_session(&self, token: &str) -> Result<()> {
        with_conflict_retry(|| async {
            self.inner
                .query("DELETE type::record('session', $key)")
                .bind(("key", hash_token(token)))
                .await
                .context("session delete failed")?
                .check()
                .context("session delete returned an error")?;
            Ok(())
        })
        .await
    }

    // ── Mutable shares + grants ─────────────────────────────────────────

    /// Create an ACCOUNT notebook (PRD-0064) and return its minted id: a
    /// share with NO published copy. The content goes straight into the draft
    /// slot, which is where the editor already writes it, so publishing later
    /// is the existing promote and nothing has to be moved between storage
    /// classes.
    ///
    /// Transactional: a share row without its OWNER grant is a notebook
    /// nobody can open, edit, or delete.
    ///
    /// This is the ONLY way a share row is born. Publishing is
    /// [`promote_draft`](Self::promote_draft) on top of it, so no path can
    /// write a row shape the ordinary save-then-publish sequence cannot
    /// produce — including from a test fixture.
    #[tracing::instrument(name = "db_create_account_notebook", level = "info", skip_all)]
    pub async fn create_account_notebook(
        &self,
        owner_github_id: &str,
        notebook_json: &str,
    ) -> Result<String> {
        with_conflict_retry(|| async {
            let id = self.mint_share_id().await?;
            self.inner
                .query(
                    "BEGIN;
                     CREATE type::record('mutable_share', $id) SET \
                        notebook_json = NONE, manifest_json = NONE, \
                        draft_json = $nb, draft_bytes = $bytes, \
                        bytes = 0, pushed_at = NONE, created_at = $now;
                     CREATE rbac_grant SET \
                        user = type::record('user', $uid), \
                        resource_kind = $kind, resource_id = $id, role = $role;
                     COMMIT;",
                )
                .bind(("id", id.clone()))
                .bind(("nb", notebook_json.to_string()))
                .bind((
                    "bytes",
                    i64::try_from(notebook_json.len()).unwrap_or(i64::MAX),
                ))
                .bind(("now", now_rfc3339()))
                .bind(("uid", owner_github_id.to_string()))
                .bind(("kind", KIND_MUTABLE_SHARE))
                .bind(("role", ROLE_OWNER))
                .await
                .context("account notebook create failed")?
                .check()
                .context("account notebook create returned an error")?;
            Ok(id)
        })
        .await
    }

    /// Mint a share id that is not already taken (the `/mutable/{id}` path
    /// segment). Collisions are astronomically unlikely at 64 bits; the loop
    /// is a belt-and-braces guard, and the transactional CREATE would reject
    /// a collision anyway.
    async fn mint_share_id(&self) -> Result<String> {
        for _ in 0..8 {
            let candidate = random_share_id();
            if !self.mutable_share_exists(&candidate).await? {
                return Ok(candidate);
            }
        }
        anyhow::bail!("failed to mint a unique mutable share id")
    }

    /// Whether a share id is taken (used when minting fresh ids).
    pub async fn mutable_share_exists(&self, id: &str) -> Result<bool> {
        // The probe reads the record id itself, which every row has. Reading
        // a CONTENT field would make an unpublished account notebook look
        // like a deserialization failure, in the one check that decides
        // whether a fresh id is free (PRD-0064).
        #[derive(SurrealValue)]
        struct Row {
            id: String,
        }
        let row: Option<Row> = self
            .inner
            .query("SELECT record::id(id) AS id FROM ONLY type::record('mutable_share', $id)")
            .bind(("id", id.to_string()))
            .await
            .context("share existence check failed")?
            .take(0)
            .context("share existence row malformed")?;
        Ok(row.is_some())
    }

    /// Fetch a share with its owner attribution, or `None` for unknown ids.
    #[tracing::instrument(name = "db_get_share", level = "info", skip_all, fields(id = %id))]
    pub async fn get_mutable_share(&self, id: &str) -> Result<Option<MutableShareRow>> {
        #[derive(SurrealValue)]
        struct ShareRowRaw {
            notebook_json: Option<String>,
            manifest_json: Option<String>,
            private: Option<bool>,
        }

        let mut response = self
            .inner
            .query(
                "SELECT notebook_json, manifest_json, private \
                 FROM ONLY type::record('mutable_share', $id);
                 SELECT record::id(user) AS github_id, user.login AS login, \
                    user.avatar_url AS avatar_url \
                 FROM rbac_grant \
                 WHERE resource_kind = $kind AND resource_id = $id AND role = $role \
                 LIMIT 1;",
            )
            .bind(("id", id.to_string()))
            .bind(("kind", KIND_MUTABLE_SHARE))
            .bind(("role", ROLE_OWNER))
            .await
            .context("share fetch failed")?;

        let Some(share) = response
            .take::<Option<ShareRowRaw>>(0)
            .context("share row malformed")?
        else {
            return Ok(None);
        };
        let owner = response
            .take::<Vec<AuthUser>>(1)
            .context("owner row malformed")?
            .into_iter()
            .next();

        Ok(Some(MutableShareRow {
            notebook_json: share.notebook_json,
            manifest_json: share.manifest_json,
            // Pre-PRD-0061 rows carry no field value; absent means public.
            private: share.private.unwrap_or(false),
            owner,
        }))
    }

    /// Overwrite a share's content (a push). Ownership is checked by the
    /// caller via [`user_owns_share`](Self::user_owns_share).
    #[tracing::instrument(name = "db_update_share", level = "info", skip_all, fields(id = %id))]
    pub async fn update_mutable_share(
        &self,
        id: &str,
        notebook_json: &str,
        manifest_json: Option<String>,
    ) -> Result<()> {
        with_conflict_retry(|| async {
            self.inner
                .query(
                    "UPDATE type::record('mutable_share', $id) SET \
                        notebook_json = $nb, manifest_json = $mf, \
                        bytes = $bytes, pushed_at = $now",
                )
                .bind(("id", id.to_string()))
                .bind(("nb", notebook_json.to_string()))
                .bind(("mf", manifest_json.clone()))
                .bind((
                    "bytes",
                    i64::try_from(notebook_json.len()).unwrap_or(i64::MAX),
                ))
                .bind(("now", now_rfc3339()))
                .await
                .context("share update failed")?
                .check()
                .context("share update returned an error")?;
            Ok(())
        })
        .await
    }

    // ── Draft slot (PRD-0054) ───────────────────────────────────────────

    /// Write the draft slot (an autosave). Ownership is checked by the
    /// caller; a nonexistent id is a silent no-op (UPDATE semantics), which
    /// cannot occur behind the ownership gate.
    #[tracing::instrument(name = "db_save_draft", level = "debug", skip_all, fields(id = %id))]
    pub async fn save_draft(&self, id: &str, notebook_json: &str) -> Result<()> {
        // draft_bytes is a Rust-computed BYTE length, cleared to NONE by
        // promote/discard. total_mutable_bytes sums it directly rather than
        // running SurrealQL string::len over draft_json, which counts CHARS
        // and undercounts multibyte drafts 3-4x — the exact bug promote_draft
        // already fixed for the published `bytes` field.
        with_conflict_retry(|| async {
            self.inner
                .query(
                    "UPDATE type::record('mutable_share', $id) \
                     SET draft_json = $nb, draft_bytes = $bytes",
                )
                .bind(("id", id.to_string()))
                .bind(("nb", notebook_json.to_string()))
                .bind((
                    "bytes",
                    i64::try_from(notebook_json.len()).unwrap_or(i64::MAX),
                ))
                .await
                .context("draft save failed")?
                .check()
                .context("draft save returned an error")?;
            Ok(())
        })
        .await
    }

    /// The owner's editing view: the draft when one exists, else published.
    #[tracing::instrument(name = "db_get_share_for_edit", level = "info", skip_all, fields(id = %id))]
    pub async fn get_share_for_edit(&self, id: &str) -> Result<Option<ShareEditRow>> {
        #[derive(SurrealValue)]
        struct Row {
            notebook_json: Option<String>,
            draft_json: Option<String>,
            private: Option<bool>,
        }
        let row: Option<Row> = self
            .inner
            .query(
                "SELECT notebook_json, draft_json, private \
                 FROM ONLY type::record('mutable_share', $id)",
            )
            .bind(("id", id.to_string()))
            .await
            .context("edit fetch failed")?
            .take(0)
            .context("edit row malformed")?;
        let Some(r) = row else { return Ok(None) };
        let published = r.notebook_json.is_some();
        let dirty = r.draft_json.is_some();
        let notebook_json = resolve_content(r.draft_json, r.notebook_json).ok_or_else(|| {
            anyhow::anyhow!("share {id} has neither a published copy nor a draft")
        })?;
        Ok(Some(ShareEditRow {
            notebook_json,
            dirty,
            published,
            private: r.private.unwrap_or(false),
        }))
    }

    /// Clear the draft slot without promoting (Discard draft).
    ///
    /// Guarded on a published copy existing: discarding the draft of an
    /// unpublished account notebook would delete its only content, and both
    /// slots empty is corruption rather than a state (PRD-0064). Such a row
    /// has nothing to revert TO, so the discard is a no-op there.
    ///
    /// Returns whether a row actually matched, so a caller can tell a real
    /// discard from that no-op. Reporting success for a write the WHERE
    /// clause declined would have the editor announce "back to the published
    /// copy" and reload over content nothing touched.
    #[tracing::instrument(name = "db_discard_draft", level = "info", skip_all, fields(id = %id))]
    pub async fn discard_draft(&self, id: &str) -> Result<bool> {
        with_conflict_retry(|| async {
            #[derive(SurrealValue)]
            struct Row {
                id: String,
            }
            let rows: Vec<Row> = self
                .inner
                .query(
                    "UPDATE type::record('mutable_share', $id) \
                     SET draft_json = NONE, draft_bytes = NONE \
                     WHERE notebook_json != NONE \
                     RETURN record::id(id) AS id",
                )
                .bind(("id", id.to_string()))
                .await
                .context("draft discard failed")?
                .take(0)
                .context("draft discard returned an error")?;
            Ok(!rows.is_empty())
        })
        .await
    }

    /// Promote the draft to published (a Push): published := draft, manifest
    /// replaced, `pushed_at` bumped, draft cleared — one statement, so a
    /// concurrent autosave either lands before (and is promoted) or after
    /// (and stays a fresh draft). No-op when no draft exists.
    ///
    /// The WHERE guard makes this promote-or-nothing; the caller decides "was
    /// there anything to push" via [`get_share_for_edit`](Self::get_share_for_edit)
    /// beforehand (a benign race, last-write-wins by design, PRD-0054).
    ///
    /// A FIRST publish (PRD-0064: `notebook_json` and `pushed_at` still NONE
    /// on an account notebook) is the same statement, not a special case:
    /// `draft_json ?? notebook_json` takes the draft, and `pushed_at` moves
    /// from NONE to now.
    ///
    /// `published_bytes` is the BYTE length of the draft the caller read
    /// (every other path stores Rust `.len()` bytes; the in-DB
    /// `string::len` this replaced counted CHARS, silently undercounting
    /// multibyte notebooks). If an autosave lands between the caller's read
    /// and this promote, the recorded size lags by one autosave — the same
    /// last-write-wins drift as the content itself, corrected on next push.
    #[tracing::instrument(name = "db_promote_draft", level = "info", skip_all, fields(id = %id))]
    pub async fn promote_draft(
        &self,
        id: &str,
        manifest_json: Option<String>,
        published_bytes: u64,
    ) -> Result<()> {
        with_conflict_retry(|| async {
            self.inner
                .query(
                    "UPDATE type::record('mutable_share', $id) SET \
                        notebook_json = draft_json ?? notebook_json, \
                        manifest_json = $mf, \
                        bytes = $bytes, \
                        pushed_at = $now, \
                        draft_json = NONE, \
                        draft_bytes = NONE \
                     WHERE draft_json != NONE",
                )
                .bind(("id", id.to_string()))
                .bind(("mf", manifest_json.clone()))
                .bind(("bytes", i64::try_from(published_bytes).unwrap_or(i64::MAX)))
                .bind(("now", now_rfc3339()))
                .await
                .context("draft promote failed")?
                .check()
                .context("draft promote returned an error")?;
            Ok(())
        })
        .await
    }

    /// Test-only: make every subsequent PUBLISH fail, and nothing else.
    ///
    /// Constrains `bytes` to the zero an unpublished row already holds, so a
    /// save still lands and [`promote_draft`](Self::promote_draft) — the only
    /// statement that writes a nonzero `bytes` — is rejected by the database.
    /// Narrow on purpose rather than a general "run this SQL" hatch: the one
    /// thing it can do is fail a publish, which is the half of Share Mutable
    /// that has to roll back the half before it.
    #[cfg(test)]
    pub(crate) async fn break_publish_for_tests(&self) -> Result<()> {
        self.inner
            .query("DEFINE FIELD OVERWRITE bytes ON mutable_share TYPE int ASSERT $value = 0")
            .await
            .context("publish-breaking DDL failed")?
            .check()
            .context("publish-breaking DDL returned an error")?;
        Ok(())
    }

    /// Unpublish in place (PRD-0064): drop the published copy and keep the
    /// notebook in the owner's account as an editable draft.
    ///
    /// A clean share's published copy is its ONLY copy, so it moves into the
    /// draft slot rather than being cleared: a row with no content anywhere is
    /// corruption, not a state. That is also what lets this replace the old
    /// delete-and-write-to-IndexedDB dance — no moment exists where the
    /// browser holds the only copy.
    ///
    /// The assignment order reads as if it mattered; it does not. `SurrealDB`
    /// evaluates every right-hand side against the PRE-update document, so
    /// `draft_json = draft_json ?? notebook_json` sees the published content
    /// whether it is written above or below `notebook_json = NONE` (measured:
    /// swapping them leaves the regression test green). What the test
    /// actually gates is that the move is there at all.
    ///
    /// Idempotent: unpublishing an already-unpublished notebook leaves the
    /// draft alone.
    #[tracing::instrument(name = "db_unpublish_share", level = "info", skip_all, fields(id = %id))]
    pub async fn unpublish_share(&self, id: &str) -> Result<()> {
        with_conflict_retry(|| async {
            self.inner
                .query(
                    "UPDATE type::record('mutable_share', $id) SET \
                        draft_json = draft_json ?? notebook_json, \
                        draft_bytes = draft_bytes ?? bytes, \
                        notebook_json = NONE, \
                        manifest_json = NONE, \
                        pushed_at = NONE, \
                        bytes = 0",
                )
                .bind(("id", id.to_string()))
                .await
                .context("share unpublish failed")?
                .check()
                .context("share unpublish returned an error")?;
            Ok(())
        })
        .await
    }

    /// Delete a share and every grant on it, in one transaction.
    #[tracing::instrument(name = "db_delete_share", level = "info", skip_all, fields(id = %id))]
    pub async fn delete_mutable_share(&self, id: &str) -> Result<()> {
        with_conflict_retry(|| async {
            self.inner
                .query(
                    "BEGIN;
                     DELETE type::record('mutable_share', $id);
                     DELETE rbac_grant WHERE resource_kind = $kind AND resource_id = $id;
                     COMMIT;",
                )
                .bind(("id", id.to_string()))
                .bind(("kind", KIND_MUTABLE_SHARE))
                .await
                .context("share delete failed")?
                .check()
                .context("share delete returned an error")?;
            Ok(())
        })
        .await
    }

    /// Whether `github_id` holds the OWNER grant on a share.
    #[tracing::instrument(name = "db_user_owns_share", level = "debug", skip_all, fields(id = %id))]
    pub async fn user_owns_share(&self, github_id: &str, id: &str) -> Result<bool> {
        #[derive(SurrealValue)]
        struct Row {
            resource_id: String,
        }
        let rows: Vec<Row> = self
            .inner
            .query(
                "SELECT resource_id FROM rbac_grant \
                 WHERE user = type::record('user', $uid) \
                    AND resource_kind = $kind AND resource_id = $id AND role = $role",
            )
            .bind(("uid", github_id.to_string()))
            .bind(("kind", KIND_MUTABLE_SHARE))
            .bind(("id", id.to_string()))
            .bind(("role", ROLE_OWNER))
            .await
            .context("ownership check failed")?
            .take(0)
            .context("ownership row malformed")?;
        Ok(!rows.is_empty())
    }

    // ── Privacy + READ grants (PRD-0061) ────────────────────────────────

    /// Flip a share's privacy flag. Ownership is checked by the caller.
    #[tracing::instrument(name = "db_set_share_private", level = "info", skip_all, fields(id = %id, private = private))]
    pub async fn set_share_private(&self, id: &str, private: bool) -> Result<()> {
        with_conflict_retry(|| async {
            self.inner
                .query("UPDATE type::record('mutable_share', $id) SET private = $private")
                .bind(("id", id.to_string()))
                .bind(("private", private))
                .await
                .context("privacy update failed")?
                .check()
                .context("privacy update returned an error")?;
            Ok(())
        })
        .await
    }

    /// Resolve a GitHub login to a user who has signed in to ironpad.
    /// Logins are matched case-insensitively (GitHub treats them that way).
    #[tracing::instrument(name = "db_find_user_by_login", level = "debug", skip_all)]
    pub async fn find_user_by_login(&self, login: &str) -> Result<Option<AuthUser>> {
        let rows: Vec<AuthUser> = self
            .inner
            .query(
                "SELECT record::id(id) AS github_id, login, avatar_url FROM user \
                 WHERE string::lowercase(login) = string::lowercase($login) LIMIT 1",
            )
            .bind(("login", login.to_string()))
            .await
            .context("user lookup failed")?
            .take(0)
            .context("user row malformed")?;
        Ok(rows.into_iter().next())
    }

    /// Mint a READ grant. Idempotent: the unique grant index rejects
    /// duplicates, which this treats as success.
    #[tracing::instrument(name = "db_grant_read", level = "info", skip_all, fields(id = %id))]
    pub async fn grant_read(&self, id: &str, github_id: &str) -> Result<()> {
        with_conflict_retry(|| async {
            let result = self
                .inner
                .query(
                    "CREATE rbac_grant SET \
                        user = type::record('user', $uid), \
                        resource_kind = $kind, resource_id = $id, role = $role",
                )
                .bind(("uid", github_id.to_string()))
                .bind(("kind", KIND_MUTABLE_SHARE))
                .bind(("id", id.to_string()))
                .bind(("role", ROLE_READ))
                .await
                .context("grant create failed")?
                .check();
            match result {
                Ok(_) => Ok(()),
                // A duplicate grant is the caller clicking twice, not an error.
                Err(e) if e.to_string().contains("grant_unique") => Ok(()),
                Err(e) => Err(e).context("grant create returned an error"),
            }
        })
        .await
    }

    /// Remove a READ grant (never the OWNER's).
    #[tracing::instrument(name = "db_revoke_read", level = "info", skip_all, fields(id = %id))]
    pub async fn revoke_read(&self, id: &str, github_id: &str) -> Result<()> {
        with_conflict_retry(|| async {
            self.inner
                .query(
                    "DELETE rbac_grant \
                     WHERE user = type::record('user', $uid) \
                        AND resource_kind = $kind AND resource_id = $id AND role = $role",
                )
                .bind(("uid", github_id.to_string()))
                .bind(("kind", KIND_MUTABLE_SHARE))
                .bind(("id", id.to_string()))
                .bind(("role", ROLE_READ))
                .await
                .context("grant revoke failed")?
                .check()
                .context("grant revoke returned an error")?;
            Ok(())
        })
        .await
    }

    /// Everyone holding a READ grant on a share, for the owner's Access UI.
    #[tracing::instrument(name = "db_list_read_grants", level = "info", skip_all, fields(id = %id))]
    pub async fn list_read_grants(&self, id: &str) -> Result<Vec<AuthUser>> {
        let rows: Vec<AuthUser> = self
            .inner
            .query(
                "SELECT record::id(user) AS github_id, user.login AS login, \
                    user.avatar_url AS avatar_url \
                 FROM rbac_grant \
                 WHERE resource_kind = $kind AND resource_id = $id AND role = $role",
            )
            .bind(("kind", KIND_MUTABLE_SHARE))
            .bind(("id", id.to_string()))
            .bind(("role", ROLE_READ))
            .await
            .context("grant listing failed")?
            .take(0)
            .context("grant rows malformed")?;
        Ok(rows)
    }

    /// May `github_id` view a PRIVATE share — OWNER or READ. (Public shares
    /// never consult this.)
    #[tracing::instrument(name = "db_user_can_read_share", level = "debug", skip_all, fields(id = %id))]
    pub async fn user_can_read_share(&self, github_id: &str, id: &str) -> Result<bool> {
        #[derive(SurrealValue)]
        struct Row {
            resource_id: String,
        }
        let rows: Vec<Row> = self
            .inner
            .query(
                "SELECT resource_id FROM rbac_grant \
                 WHERE user = type::record('user', $uid) \
                    AND resource_kind = $kind AND resource_id = $id \
                    AND (role = $owner OR role = $read)",
            )
            .bind(("uid", github_id.to_string()))
            .bind(("kind", KIND_MUTABLE_SHARE))
            .bind(("id", id.to_string()))
            .bind(("owner", ROLE_OWNER))
            .bind(("read", ROLE_READ))
            .await
            .context("read-access check failed")?
            .take(0)
            .context("read-access row malformed")?;
        Ok(!rows.is_empty())
    }

    /// The ids of every share a user holds the OWNER grant on. The one place
    /// "which notebooks are mine" is expressed, so the listing and the
    /// per-user byte total cannot drift apart.
    async fn owned_share_ids(&self, github_id: &str) -> Result<Vec<String>> {
        #[derive(SurrealValue)]
        struct GrantRow {
            resource_id: String,
        }
        let grants: Vec<GrantRow> = self
            .inner
            .query(
                "SELECT resource_id FROM rbac_grant \
                 WHERE user = type::record('user', $uid) \
                    AND resource_kind = $kind AND role = $role",
            )
            .bind(("uid", github_id.to_string()))
            .bind(("kind", KIND_MUTABLE_SHARE))
            .bind(("role", ROLE_OWNER))
            .await
            .context("grant listing failed")?
            .take(0)
            .context("grant rows malformed")?;
        Ok(grants.into_iter().map(|g| g.resource_id).collect())
    }

    /// Every share a user owns, published or not (PRD-0064), newest activity
    /// first. Content is draft-or-published, so an unpublished notebook
    /// carries the copy that actually exists.
    #[tracing::instrument(name = "db_list_owned_shares", level = "info", skip_all)]
    pub async fn list_shares_owned_by(&self, github_id: &str) -> Result<Vec<OwnedShareRow>> {
        #[derive(SurrealValue)]
        struct Row {
            id: String,
            notebook_json: Option<String>,
            draft_json: Option<String>,
            pushed_at: Option<String>,
            created_at: String,
        }
        let ids = self.owned_share_ids(github_id).await?;
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let rows: Vec<Row> = self
            .inner
            .query(
                "SELECT record::id(id) AS id, notebook_json, draft_json, \
                    pushed_at, created_at \
                 FROM mutable_share WHERE record::id(id) IN $ids",
            )
            .bind(("ids", ids))
            .await
            .context("owned share listing failed")?
            .take(0)
            .context("owned share rows malformed")?;

        let mut out: Vec<OwnedShareRow> = rows
            .into_iter()
            .filter_map(|r| {
                let published = r.notebook_json.is_some();
                let Some(notebook_json) = resolve_content(r.draft_json, r.notebook_json) else {
                    // Corruption, not a state. Drop the row rather than
                    // failing the call: one unreadable notebook must not hide
                    // every other notebook the user owns.
                    tracing::warn!(id = %r.id, "share has neither a published copy nor a draft");
                    return None;
                };
                Some(OwnedShareRow {
                    id: r.id,
                    notebook_json,
                    pushed_at: r.pushed_at,
                    created_at: r.created_at,
                    published,
                })
            })
            .collect();
        out.sort_by(|a, b| b.last_activity().cmp(a.last_activity()));
        Ok(out)
    }

    /// Total bytes of stored notebook JSON across all shares, DRAFTS
    /// INCLUDED (the aggregate cap input; blobs are capped separately by the
    /// blob store). Counting `draft_json` matters: autosaves are the one
    /// write path an owner can drive at will, and an uncounted draft slot
    /// would let stored bytes grow unboundedly past the cap.
    pub async fn total_mutable_bytes(&self) -> Result<u64> {
        #[derive(SurrealValue)]
        struct Row {
            total: i64,
        }
        let row: Option<Row> = self
            .inner
            .query(
                "SELECT math::sum(bytes + (draft_bytes ?? 0)) AS total \
                 FROM mutable_share GROUP ALL",
            )
            .await
            .context("byte total failed")?
            .take(0)
            .context("byte total row malformed")?;
        Ok(row.map_or(0, |r| u64::try_from(r.total).unwrap_or(0)))
    }

    /// The same total, narrowed to the shares ONE user owns (PRD-0064): the
    /// per-user cap input. Storing notebooks is the point of an account, so
    /// the instance-wide cap alone would let one account consume everyone
    /// else's room.
    #[tracing::instrument(name = "db_user_bytes", level = "debug", skip_all)]
    pub async fn total_mutable_bytes_for_user(&self, github_id: &str) -> Result<u64> {
        #[derive(SurrealValue)]
        struct Row {
            total: i64,
        }
        let ids = self.owned_share_ids(github_id).await?;
        if ids.is_empty() {
            return Ok(0);
        }
        let row: Option<Row> = self
            .inner
            .query(
                "SELECT math::sum(bytes + (draft_bytes ?? 0)) AS total \
                 FROM mutable_share WHERE record::id(id) IN $ids GROUP ALL",
            )
            .bind(("ids", ids))
            .await
            .context("per-user byte total failed")?
            .take(0)
            .context("per-user byte total row malformed")?;
        Ok(row.map_or(0, |r| u64::try_from(r.total).unwrap_or(0)))
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────────

/// Content resolution (PRD-0064): the draft when one exists, else the
/// published copy. The ONE place the rule is written, so the editor view and
/// the account listing cannot disagree about what a notebook's content is.
///
/// `None` means the row carries neither, which the account invariant forbids.
fn resolve_content(draft_json: Option<String>, notebook_json: Option<String>) -> Option<String> {
    draft_json.or(notebook_json)
}

/// A fresh share id: the `/mutable/{id}` path segment.
fn random_share_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()[..16].to_string()
}

/// A fresh 256-bit session token as lowercase hex — the cookie value.
fn random_token() -> String {
    use rand::RngCore as _;
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    hex(&bytes)
}

/// blake3 of a session token, as the session record key. Tokens are
/// full-entropy, so a plain (unsalted) hash is enumeration-proof, same
/// reasoning as the PRD-0049 key hashes this replaces.
fn hash_token(token: &str) -> String {
    blake3::hash(token.as_bytes()).to_hex().to_string()
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        })
}

fn now_secs() -> i64 {
    chrono::Utc::now().timestamp()
}

fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339()
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// A PUBLISHED share, minted the only way production mints one: an
    /// account notebook, promoted by a push. Returns the id the server
    /// chose. Fixtures that wrote a published row directly used to set
    /// `bytes`/`pushed_at` at create time, which no code path does any more —
    /// so tests could stay green against a row shape production cannot
    /// produce.
    async fn published_share(
        db: &Db,
        owner_github_id: &str,
        notebook_json: &str,
        manifest_json: Option<String>,
    ) -> String {
        let id = db
            .create_account_notebook(owner_github_id, notebook_json)
            .await
            .unwrap();
        db.promote_draft(&id, manifest_json, notebook_json.len() as u64)
            .await
            .unwrap();
        id
    }

    async fn test_db() -> (tempfile::TempDir, Db) {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("test.db")).await.unwrap();
        (dir, db)
    }

    #[tokio::test]
    async fn schema_definition_is_idempotent() {
        // Every boot re-runs the DEFINEs against the existing file; IF NOT
        // EXISTS makes that safe. (A true close-and-reopen can't be tested
        // in-process: SurrealKV's file lock outlives the dropped handle.)
        let (_dir, db) = test_db().await;
        db.define_schema().await.unwrap();
        db.define_schema().await.unwrap();
    }

    #[tokio::test]
    async fn instance_counts_counts_rows() {
        let (_dir, db) = test_db().await;
        assert_eq!(db.instance_counts().await.unwrap(), (0, 0, 0), "empty db");

        db.upsert_user("42", "octocat", "https://e.com/a.png")
            .await
            .unwrap();
        let _token = db.create_session("42").await.unwrap();

        let (users, sessions, shares) = db.instance_counts().await.unwrap();
        assert_eq!((users, sessions, shares), (1, 1, 0));
    }

    #[tokio::test]
    async fn admin_user_list_carries_per_user_counts() {
        let (_dir, db) = test_db().await;
        db.upsert_user("1", "alice", "https://e.com/a.png")
            .await
            .unwrap();
        db.upsert_user("2", "bob", "https://e.com/b.png")
            .await
            .unwrap();

        let _t1 = db.create_session("1").await.unwrap();
        let _t2 = db.create_session("1").await.unwrap();
        published_share(&db, "1", "{}", None).await;

        let users = db.list_users_for_admin().await.unwrap();
        assert_eq!(users.len(), 2);

        let alice = users.iter().find(|u| u.login == "alice").unwrap();
        assert_eq!(alice.sessions, 2, "two sessions minted");
        assert_eq!(alice.owned_shares, 1);

        // A user with nothing must appear with zeroes, not be missing: the
        // counts come from grouped queries that only return rows for users
        // who have something.
        let bob = users.iter().find(|u| u.login == "bob").unwrap();
        assert_eq!(bob.sessions, 0);
        assert_eq!(bob.owned_shares, 0);
    }

    #[tokio::test]
    async fn revoking_sessions_signs_out_only_that_user() {
        let (_dir, db) = test_db().await;
        db.upsert_user("1", "alice", "").await.unwrap();
        db.upsert_user("2", "bob", "").await.unwrap();
        let alice_token = db.create_session("1").await.unwrap();
        let bob_token = db.create_session("2").await.unwrap();

        assert_eq!(db.revoke_user_sessions("1").await.unwrap(), 1);
        assert!(
            db.session_user(&alice_token).await.unwrap().is_none(),
            "alice is signed out"
        );
        assert!(
            db.session_user(&bob_token).await.unwrap().is_some(),
            "bob is untouched"
        );

        // Idempotent: revoking again removes nothing and is not an error.
        assert_eq!(db.revoke_user_sessions("1").await.unwrap(), 0);

        // The user itself survives; this is a sign-out, not a deletion.
        let users = db.list_users_for_admin().await.unwrap();
        assert!(users.iter().any(|u| u.login == "alice"));
    }

    #[tokio::test]
    async fn admin_pin_is_trust_on_first_use() {
        let (_dir, db) = test_db().await;
        assert_eq!(db.admin_pin().await.unwrap(), None, "nothing pinned yet");

        assert_eq!(db.pin_admin("42").await.unwrap(), "42");
        assert_eq!(db.admin_pin().await.unwrap().as_deref(), Some("42"));

        // A second identity does NOT take over. This is the whole point: a
        // GitHub login can be renamed and the freed handle claimed by someone
        // else, who would then match a login allowlist under a different id.
        assert_eq!(
            db.pin_admin("99").await.unwrap(),
            "42",
            "an existing pin must win"
        );
        assert_eq!(db.admin_pin().await.unwrap().as_deref(), Some("42"));
    }

    #[tokio::test]
    async fn session_round_trip_and_logout() {
        let (_dir, db) = test_db().await;
        db.upsert_user("42", "octocat", "https://example.com/a.png")
            .await
            .unwrap();

        let token = db.create_session("42").await.unwrap();
        assert_eq!(token.len(), 64, "32 random bytes as hex");

        let (user, renewed) = db
            .session_user(&token)
            .await
            .unwrap()
            .expect("live session");
        assert_eq!(user.github_id, "42");
        assert_eq!(user.login, "octocat");
        assert_eq!(user.avatar_url, "https://example.com/a.png");
        assert!(!renewed, "a fresh session must not renew immediately");

        // A bogus token resolves to nobody.
        assert!(db.session_user(&random_token()).await.unwrap().is_none());

        db.delete_session(&token).await.unwrap();
        assert!(db.session_user(&token).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn session_renewal_slides_and_reports_exactly_when_it_happens() {
        let (_dir, db) = test_db().await;
        db.upsert_user("9", "renewer", "").await.unwrap();
        let token = db.create_session("9").await.unwrap();

        // Age the session just past the renewal threshold (nowhere near
        // expiry): the next lookup must slide it AND say so, because the
        // caller re-issues the cookie's Max-Age off that flag.
        db.inner
            .query("UPDATE session SET expires_at = $exp")
            .bind((
                "exp",
                now_secs() + SESSION_TTL_SECS - SESSION_RENEW_AFTER_SECS - 60,
            ))
            .await
            .unwrap()
            .check()
            .unwrap();

        let (_, renewed) = db.session_user(&token).await.unwrap().unwrap();
        assert!(renewed, "an aged session slides");
        // The slide is throttled: the immediately-following lookup is fresh.
        let (_, renewed) = db.session_user(&token).await.unwrap().unwrap();
        assert!(!renewed, "renewal happens at most once per threshold");
    }

    #[tokio::test]
    async fn upsert_user_refreshes_identity_but_keeps_created_at() {
        let (_dir, db) = test_db().await;
        db.upsert_user("7", "old-login", "https://a/1.png")
            .await
            .unwrap();
        // A rename on GitHub flows through on next login.
        db.upsert_user("7", "new-login", "https://a/2.png")
            .await
            .unwrap();

        let token = db.create_session("7").await.unwrap();
        let (user, _) = db.session_user(&token).await.unwrap().unwrap();
        assert_eq!(user.login, "new-login");
        assert_eq!(user.avatar_url, "https://a/2.png");
    }

    #[tokio::test]
    async fn share_lifecycle_with_owner() {
        let (_dir, db) = test_db().await;
        db.upsert_user("1", "author", "https://a/author.png")
            .await
            .unwrap();
        db.upsert_user("2", "rando", "https://a/rando.png")
            .await
            .unwrap();

        let id = published_share(&db, "1", "{\"title\":\"nb\"}", None).await;

        assert!(db.mutable_share_exists(&id).await.unwrap());
        assert!(!db.mutable_share_exists("ffffffffffffffff").await.unwrap());

        // Owner attribution is resolved with the share.
        let row = db
            .get_mutable_share(&id)
            .await
            .unwrap()
            .expect("share exists");
        assert_eq!(row.notebook_json.as_deref(), Some("{\"title\":\"nb\"}"));
        assert!(row.manifest_json.is_none());
        let owner = row.owner.expect("owner grant resolved");
        assert_eq!(owner.login, "author");
        assert_eq!(owner.github_id, "1");

        // RBAC: the author owns it; the rando does not.
        assert!(db.user_owns_share("1", &id).await.unwrap());
        assert!(!db.user_owns_share("2", &id).await.unwrap());

        // Push updates content + manifest.
        db.update_mutable_share(
            &id,
            "{\"title\":\"nb2\"}",
            Some("{\"version\":1}".to_string()),
        )
        .await
        .unwrap();
        let row = db.get_mutable_share(&id).await.unwrap().unwrap();
        assert_eq!(row.notebook_json.as_deref(), Some("{\"title\":\"nb2\"}"));
        assert_eq!(row.manifest_json.as_deref(), Some("{\"version\":1}"));

        // Delete removes the share AND its grant.
        db.delete_mutable_share(&id).await.unwrap();
        assert!(db.get_mutable_share(&id).await.unwrap().is_none());
        assert!(!db.user_owns_share("1", &id).await.unwrap());
    }

    #[tokio::test]
    async fn saving_twice_mints_two_rows_rather_than_overwriting_one() {
        // The id is server-minted now, so "reject a duplicate id" is no
        // longer a caller-visible contract; what replaced it is that a second
        // save of identical bytes cannot land on top of the first.
        let (_dir, db) = test_db().await;
        db.upsert_user("1", "author", "https://a/a.png")
            .await
            .unwrap();
        let first = db.create_account_notebook("1", "{\"v\":1}").await.unwrap();
        let second = db.create_account_notebook("1", "{\"v\":2}").await.unwrap();
        assert_ne!(first, second);
        assert_eq!(
            db.get_share_for_edit(&first)
                .await
                .unwrap()
                .unwrap()
                .notebook_json,
            "{\"v\":1}",
            "the first notebook must survive the second save"
        );
        assert_eq!(db.list_shares_owned_by("1").await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn list_owned_shares_newest_first() {
        let (_dir, db) = test_db().await;
        db.upsert_user("1", "author", "https://a/a.png")
            .await
            .unwrap();
        db.upsert_user("2", "other", "https://a/o.png")
            .await
            .unwrap();

        published_share(&db, "1", "{\"a\":1}", None).await;
        published_share(&db, "1", "{\"b\":2}", None).await;
        let theirs = published_share(&db, "2", "{\"c\":3}", None).await;

        let mine = db.list_shares_owned_by("1").await.unwrap();
        assert_eq!(mine.len(), 2);
        assert!(mine.iter().all(|s| s.id != theirs));
        // Newest push first (b was created after a).
        assert!(mine[0].last_activity() >= mine[1].last_activity());
        assert!(mine.iter().all(|s| s.published), "both were published");

        assert!(db.list_shares_owned_by("999").await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn total_mutable_bytes_sums_notebook_json_and_drafts() {
        let (_dir, db) = test_db().await;
        db.upsert_user("1", "author", "https://a/a.png")
            .await
            .unwrap();
        assert_eq!(db.total_mutable_bytes().await.unwrap(), 0);

        let a = published_share(&db, "1", "12345", None).await;
        let b = published_share(&db, "1", "1234567890", None).await;
        assert_eq!(db.total_mutable_bytes().await.unwrap(), 15);

        // A draft counts toward the total (it is owner-drivable storage)...
        db.save_draft(&a, "123456789012345678901234567890")
            .await
            .unwrap();
        assert_eq!(db.total_mutable_bytes().await.unwrap(), 45);
        // ...an overwrite replaces rather than accumulates...
        db.save_draft(&a, "12345678901234567890").await.unwrap();
        assert_eq!(db.total_mutable_bytes().await.unwrap(), 35);
        // ...and both promote and discard return it to published-only.
        db.promote_draft(&a, None, 20).await.unwrap();
        assert_eq!(db.total_mutable_bytes().await.unwrap(), 30);
        db.save_draft(&b, "xx").await.unwrap();
        assert!(db.discard_draft(&b).await.unwrap());
        assert_eq!(db.total_mutable_bytes().await.unwrap(), 30);

        // A multibyte draft counts BYTES, not chars: "日本語" is 3 chars but
        // 9 bytes. The old SurrealQL string::len returned 3, undercounting
        // the cap 3x for CJK/emoji drafts.
        db.save_draft(&b, "日本語").await.unwrap();
        assert_eq!(db.total_mutable_bytes().await.unwrap(), 30 + 9);
    }

    #[tokio::test]
    async fn draft_lifecycle_save_promote_discard() {
        let (_dir, db) = test_db().await;
        db.upsert_user("1", "author", "").await.unwrap();
        let id = published_share(&db, "1", "{\"v\":1}", None).await;

        // Fresh share: clean, and the edit view serves the published copy.
        let edit = db
            .get_share_for_edit(&id)
            .await
            .unwrap()
            .expect("share exists");
        assert!(!edit.dirty);
        assert_eq!(edit.notebook_json, "{\"v\":1}");
        // Unknown id: None, not an error.
        assert!(db
            .get_share_for_edit("ffffffffffffffff")
            .await
            .unwrap()
            .is_none());

        // An autosave lands in the draft slot; readers keep seeing published.
        db.save_draft(&id, "{\"v\":2}").await.unwrap();
        let edit = db.get_share_for_edit(&id).await.unwrap().unwrap();
        assert!(edit.dirty);
        assert_eq!(edit.notebook_json, "{\"v\":2}");
        let reader = db.get_mutable_share(&id).await.unwrap().unwrap();
        assert_eq!(
            reader.notebook_json.as_deref(),
            Some("{\"v\":1}"),
            "readers see published"
        );

        // Promote: published := draft, manifest replaced, clean again.
        db.promote_draft(&id, Some("{\"m\":1}".into()), 7)
            .await
            .unwrap();
        let reader = db.get_mutable_share(&id).await.unwrap().unwrap();
        assert_eq!(reader.notebook_json.as_deref(), Some("{\"v\":2}"));
        assert_eq!(reader.manifest_json.as_deref(), Some("{\"m\":1}"));
        assert!(!db.get_share_for_edit(&id).await.unwrap().unwrap().dirty);

        // Promote with no draft: a no-op (published and manifest untouched).
        db.promote_draft(&id, None, 0).await.unwrap();
        let reader = db.get_mutable_share(&id).await.unwrap().unwrap();
        assert_eq!(reader.notebook_json.as_deref(), Some("{\"v\":2}"));
        assert_eq!(reader.manifest_json.as_deref(), Some("{\"m\":1}"));

        // Discard: the draft evaporates; published is untouched.
        db.save_draft(&id, "{\"v\":3}").await.unwrap();
        assert!(
            db.discard_draft(&id).await.unwrap(),
            "a published share's draft is a real discard"
        );
        let edit = db.get_share_for_edit(&id).await.unwrap().unwrap();
        assert!(!edit.dirty);
        assert_eq!(edit.notebook_json, "{\"v\":2}");
    }

    #[test]
    fn tokens_are_unique_and_hashed_keys_differ_from_tokens() {
        let a = random_token();
        let b = random_token();
        assert_ne!(a, b);
        assert_ne!(
            hash_token(&a),
            a,
            "the stored key must not be the cookie value"
        );
    }

    // ── PRD-0064 T-002: account notebooks ───────────────────────────────

    #[tokio::test]
    async fn an_account_notebook_is_created_unpublished_with_its_content_in_the_draft() {
        let (_dir, db) = test_db().await;
        db.upsert_user("1", "author", "").await.unwrap();

        let id = db.create_account_notebook("1", "{\"v\":1}").await.unwrap();
        assert_eq!(id.len(), 16, "the id is the /mutable/{{id}} path segment");
        assert_ne!(
            id,
            db.create_account_notebook("1", "{\"v\":1}").await.unwrap(),
            "each save mints its own id"
        );

        // The row exists but publishes nothing.
        let row = db
            .get_mutable_share(&id)
            .await
            .unwrap()
            .expect("row exists");
        assert_eq!(row.notebook_json, None, "nothing published yet");
        assert_eq!(row.manifest_json, None, "no blob snapshot without a push");
        assert_eq!(row.owner.map(|o| o.login), Some("author".to_string()));

        // The owner's editing view resolves the draft as the content.
        let edit = db.get_share_for_edit(&id).await.unwrap().unwrap();
        assert_eq!(edit.notebook_json, "{\"v\":1}");
        assert!(!edit.published, "no published copy exists");
        assert!(
            edit.dirty,
            "an unpublished notebook is permanently dirty, which is what arms Push"
        );

        // The OWNER grant rode the same transaction: a share nobody owns is
        // unopenable and undeletable.
        assert!(db.user_owns_share("1", &id).await.unwrap());
        assert!(!db.user_owns_share("2", &id).await.unwrap());

        // The id-collision probe must see it. It used to SELECT a content
        // field, which an unpublished row would have failed to deserialize —
        // in the one check that decides whether a fresh id is free.
        assert!(db.mutable_share_exists(&id).await.unwrap());

        // Metered like any other stored notebook.
        assert_eq!(db.total_mutable_bytes_for_user("1").await.unwrap(), 14);

        // The listing (the one surviving consumer of pushed_at) dates it by
        // creation, because it has never been pushed.
        let listed = db.list_shares_owned_by("1").await.unwrap();
        let mine = listed.iter().find(|r| r.id == id).unwrap();
        assert_eq!(mine.pushed_at, None, "never pushed");
        assert_eq!(mine.last_activity(), mine.created_at);
    }

    #[tokio::test]
    async fn unpublish_moves_the_published_copy_into_an_empty_draft_slot() {
        // The case that could destroy a notebook: a clean share's published
        // copy is its ONLY copy, so clearing notebook_json without moving it
        // first loses the content outright.
        let (_dir, db) = test_db().await;
        db.upsert_user("1", "author", "").await.unwrap();
        let id = published_share(&db, "1", "{\"v\":1}", Some("{\"m\":1}".to_string())).await;
        assert!(
            !db.get_share_for_edit(&id).await.unwrap().unwrap().dirty,
            "the fixture must be CLEAN: with a draft present this test proves nothing"
        );

        db.unpublish_share(&id).await.unwrap();

        let row = db
            .get_mutable_share(&id)
            .await
            .unwrap()
            .expect("the row stays in the account");
        assert_eq!(row.notebook_json, None, "readers see nothing");
        assert_eq!(
            row.manifest_json, None,
            "a kept manifest would advertise blobs for content nobody can fetch"
        );
        assert_eq!(
            db.list_shares_owned_by("1").await.unwrap()[0].pushed_at,
            None,
            "the listing stops dating it by a publish that no longer stands"
        );

        let edit = db.get_share_for_edit(&id).await.unwrap().unwrap();
        assert_eq!(edit.notebook_json, "{\"v\":1}", "the content survived");
        assert!(!edit.published);
        assert!(edit.dirty);
        // The bytes moved with the content: neither lost nor double-counted.
        assert_eq!(db.total_mutable_bytes().await.unwrap(), 7);
        assert_eq!(db.total_mutable_bytes_for_user("1").await.unwrap(), 7);

        // The owner keeps it, and the id does not move.
        assert!(db.user_owns_share("1", &id).await.unwrap());
    }

    #[tokio::test]
    async fn unpublish_keeps_the_newer_draft_and_is_idempotent() {
        let (_dir, db) = test_db().await;
        db.upsert_user("1", "author", "").await.unwrap();
        let id = published_share(&db, "1", "{\"v\":1}", None).await;
        db.save_draft(&id, "{\"version\":2}").await.unwrap();

        db.unpublish_share(&id).await.unwrap();
        let edit = db.get_share_for_edit(&id).await.unwrap().unwrap();
        assert_eq!(
            edit.notebook_json, "{\"version\":2}",
            "unpublishing must not restore the older published copy over pending edits"
        );
        assert_eq!(db.total_mutable_bytes().await.unwrap(), 13, "draft only");

        // Unpublishing an already-unpublished notebook changes nothing.
        db.unpublish_share(&id).await.unwrap();
        let edit = db.get_share_for_edit(&id).await.unwrap().unwrap();
        assert_eq!(edit.notebook_json, "{\"version\":2}");
        assert!(!edit.published);
    }

    #[tokio::test]
    async fn first_publish_promotes_the_draft_and_sets_pushed_at_from_none() {
        let (_dir, db) = test_db().await;
        db.upsert_user("1", "author", "").await.unwrap();
        let id = db.create_account_notebook("1", "{\"v\":1}").await.unwrap();

        db.promote_draft(&id, Some("{\"m\":1}".to_string()), 7)
            .await
            .unwrap();

        let row = db.get_mutable_share(&id).await.unwrap().unwrap();
        assert_eq!(row.notebook_json.as_deref(), Some("{\"v\":1}"));
        assert_eq!(row.manifest_json.as_deref(), Some("{\"m\":1}"));
        assert!(
            db.list_shares_owned_by("1").await.unwrap()[0]
                .pushed_at
                .is_some(),
            "the first publish is what mints pushed_at"
        );
        let edit = db.get_share_for_edit(&id).await.unwrap().unwrap();
        assert!(edit.published);
        assert!(!edit.dirty, "promote clears the draft");
        assert_eq!(db.total_mutable_bytes_for_user("1").await.unwrap(), 7);

        // ...and the same id round-trips back out of published and in again.
        db.unpublish_share(&id).await.unwrap();
        assert!(!db.get_share_for_edit(&id).await.unwrap().unwrap().published);
        db.promote_draft(&id, None, 7).await.unwrap();
        let edit = db.get_share_for_edit(&id).await.unwrap().unwrap();
        assert!(edit.published);
        assert_eq!(edit.notebook_json, "{\"v\":1}");
    }

    #[tokio::test]
    async fn discarding_the_draft_of_an_unpublished_notebook_cannot_empty_it() {
        let (_dir, db) = test_db().await;
        db.upsert_user("1", "author", "").await.unwrap();
        let id = db.create_account_notebook("1", "{\"v\":1}").await.unwrap();

        // There is nothing to revert TO: the draft is the notebook.
        assert!(
            !db.discard_draft(&id).await.unwrap(),
            "a declined discard must SAY it declined: reporting success has \
             the editor announce a revert and reload over untouched content"
        );
        let edit = db.get_share_for_edit(&id).await.unwrap().unwrap();
        assert_eq!(
            edit.notebook_json, "{\"v\":1}",
            "content survives a discard"
        );

        // A published share still discards exactly as before.
        let published = published_share(&db, "1", "{\"v\":9}", None).await;
        db.save_draft(&published, "{\"v\":10}").await.unwrap();
        assert!(db.discard_draft(&published).await.unwrap());
        let edit = db.get_share_for_edit(&published).await.unwrap().unwrap();
        assert!(!edit.dirty);
        assert_eq!(edit.notebook_json, "{\"v\":9}");

        // ...and a discard against an id that does not exist at all is a
        // decline too, not a success.
        assert!(!db.discard_draft("ffffffffffffffff").await.unwrap());
    }

    #[tokio::test]
    async fn the_owned_listing_carries_unpublished_notebooks_and_flags_the_published_ones() {
        let (_dir, db) = test_db().await;
        db.upsert_user("1", "author", "").await.unwrap();
        db.upsert_user("2", "other", "").await.unwrap();

        published_share(&db, "1", "{\"a\":1}", None).await;
        let dirty_id = published_share(&db, "1", "{\"b\":1}", None).await;
        db.save_draft(&dirty_id, "{\"b\":2}").await.unwrap();
        let account = db.create_account_notebook("1", "{\"c\":1}").await.unwrap();
        let theirs = published_share(&db, "2", "{\"d\":1}", None).await;

        let mine = db.list_shares_owned_by("1").await.unwrap();
        assert_eq!(mine.len(), 3, "the unpublished notebook is listed too");
        assert!(mine.iter().all(|s| s.id != theirs));

        // Newest activity first: the account notebook was saved last and has
        // no pushed_at at all, which must not sink it below the published rows.
        assert_eq!(mine[0].id, account);
        assert!(!mine[0].published);
        assert_eq!(
            mine[0].notebook_json, "{\"c\":1}",
            "an unpublished row is titled from its draft"
        );
        assert_eq!(mine[0].last_activity(), mine[0].created_at);

        let dirty = mine.iter().find(|s| s.id == dirty_id).unwrap();
        assert!(dirty.published);
        assert_eq!(
            dirty.notebook_json, "{\"b\":2}",
            "content resolution is draft-first, published or not"
        );
        assert_eq!(dirty.last_activity(), dirty.pushed_at.as_deref().unwrap());

        assert!(db.list_shares_owned_by("999").await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn per_user_byte_totals_are_scoped_to_the_owner() {
        let (_dir, db) = test_db().await;
        db.upsert_user("1", "author", "").await.unwrap();
        db.upsert_user("2", "other", "").await.unwrap();
        assert_eq!(db.total_mutable_bytes_for_user("1").await.unwrap(), 0);

        let mine = published_share(&db, "1", "12345", None).await;
        db.save_draft(&mine, "1234567890").await.unwrap();
        db.create_account_notebook("2", "abc").await.unwrap();

        assert_eq!(
            db.total_mutable_bytes_for_user("1").await.unwrap(),
            15,
            "published plus draft, same expression as the global cap"
        );
        assert_eq!(db.total_mutable_bytes_for_user("2").await.unwrap(), 3);
        assert_eq!(
            db.total_mutable_bytes().await.unwrap(),
            18,
            "the instance-wide total still sees both"
        );
        // One account filling its allowance must not charge anyone else.
        assert_eq!(db.total_mutable_bytes_for_user("999").await.unwrap(), 0);
    }

    #[tokio::test]
    async fn a_row_with_no_content_anywhere_is_reported_rather_than_read_as_empty() {
        // Both slots NONE is corruption, not a state. No code path produces
        // it; if one ever does, the editor must refuse rather than open an
        // empty notebook over the top of it.
        let (_dir, db) = test_db().await;
        db.upsert_user("1", "author", "").await.unwrap();
        let id = db.create_account_notebook("1", "{\"v\":1}").await.unwrap();
        db.inner
            .query(
                "UPDATE type::record('mutable_share', $id) \
                 SET draft_json = NONE, draft_bytes = NONE",
            )
            .bind(("id", id.clone()))
            .await
            .unwrap()
            .check()
            .unwrap();

        assert!(
            db.get_share_for_edit(&id).await.is_err(),
            "the editing view must report it"
        );
        // The listing drops the bad row instead of failing wholesale: one
        // unreadable notebook must not hide every other one the user owns.
        let ok = published_share(&db, "1", "{\"ok\":1}", None).await;
        let mine = db.list_shares_owned_by("1").await.unwrap();
        assert_eq!(mine.len(), 1);
        assert_eq!(mine[0].id, ok);
    }

    // ── PRD-0064 T-001: widening a live SCHEMAFULL field ────────────────

    /// The `mutable_share` schema exactly as it shipped before PRD-0064:
    /// `notebook_json` and `pushed_at` are required `string`s, and every field
    /// carries `IF NOT EXISTS`.
    ///
    /// Kept verbatim here rather than reused from `define_schema`, because the
    /// premise is a database that predates the widening. A test that started
    /// from the current DDL would open a database that already has the new
    /// definition and could never fail.
    const PRE_0064_DDL: &str = "
        DEFINE TABLE IF NOT EXISTS user SCHEMAFULL;
        DEFINE FIELD IF NOT EXISTS login ON user TYPE string;
        DEFINE FIELD IF NOT EXISTS avatar_url ON user TYPE string;
        DEFINE FIELD IF NOT EXISTS created_at ON user TYPE string;

        DEFINE TABLE IF NOT EXISTS mutable_share SCHEMAFULL;
        DEFINE FIELD IF NOT EXISTS notebook_json ON mutable_share TYPE string;
        DEFINE FIELD IF NOT EXISTS manifest_json ON mutable_share TYPE option<string>;
        DEFINE FIELD IF NOT EXISTS draft_json ON mutable_share TYPE option<string>;
        DEFINE FIELD IF NOT EXISTS draft_bytes ON mutable_share TYPE option<int>;
        DEFINE FIELD IF NOT EXISTS private ON mutable_share TYPE bool DEFAULT false;
        DEFINE FIELD IF NOT EXISTS bytes ON mutable_share TYPE int;
        DEFINE FIELD IF NOT EXISTS pushed_at ON mutable_share TYPE string;
        DEFINE FIELD IF NOT EXISTS created_at ON mutable_share TYPE string;

        DEFINE TABLE IF NOT EXISTS rbac_grant SCHEMAFULL;
        DEFINE FIELD IF NOT EXISTS user ON rbac_grant TYPE record<user>;
        DEFINE FIELD IF NOT EXISTS resource_kind ON rbac_grant TYPE string;
        DEFINE FIELD IF NOT EXISTS resource_id ON rbac_grant TYPE string;
        DEFINE FIELD IF NOT EXISTS role ON rbac_grant TYPE string;
    ";

    /// The same widening written the way every other field in `define_schema`
    /// is written. This is the trap PRD-0064 names, not a straw man: it is
    /// what the file would have said if the widening had been added by
    /// copying the line above it.
    const WIDEN_WITH_IF_NOT_EXISTS: &str = "
        DEFINE FIELD IF NOT EXISTS notebook_json ON mutable_share TYPE option<string>;
        DEFINE FIELD IF NOT EXISTS pushed_at ON mutable_share TYPE option<string>;
    ";

    /// Open the file WITHOUT `Db::open`'s schema pass, so the test alone
    /// decides which DDL this database has ever seen.
    async fn bare_db(dir: &tempfile::TempDir) -> Db {
        let inner = Surreal::new::<SurrealKv>(dir.path().join("test.db"))
            .await
            .unwrap();
        inner.use_ns("ironpad").use_db("ironpad").await.unwrap();
        Db { inner }
    }

    async fn run_sql(db: &Db, sql: &str) -> Result<()> {
        db.inner.query(sql).await?.check()?;
        Ok(())
    }

    /// The id of the pre-0064 published row every migration test starts from.
    const PRE_0064_ID: &str = "aaaaaaaaaaaaaaaa";

    /// A pre-0064 database holding one published share, written as the OLD
    /// code wrote it: `notebook_json` and `pushed_at` populated at CREATE
    /// time. Spelled out here rather than routed through a `Db` method,
    /// because the premise is a row shape today's code cannot produce — the
    /// only way to make one is to write it by hand.
    async fn pre_0064_db_with_a_published_share(dir: &tempfile::TempDir) -> Db {
        let db = bare_db(dir).await;
        run_sql(&db, PRE_0064_DDL).await.unwrap();
        db.upsert_user("1", "author", "https://a/a.png")
            .await
            .unwrap();
        db.inner
            .query(
                "BEGIN;
                 CREATE type::record('mutable_share', $id) SET \
                    notebook_json = $nb, manifest_json = $mf, \
                    bytes = 7, pushed_at = $now, created_at = $now;
                 CREATE rbac_grant SET \
                    user = type::record('user', '1'), \
                    resource_kind = $kind, resource_id = $id, role = $role;
                 COMMIT;",
            )
            .bind(("id", PRE_0064_ID))
            .bind(("nb", "{\"v\":1}"))
            .bind(("mf", "{\"m\":1}"))
            .bind(("now", now_rfc3339()))
            .bind(("kind", KIND_MUTABLE_SHARE))
            .bind(("role", ROLE_OWNER))
            .await
            .unwrap()
            .check()
            .unwrap();
        db
    }

    /// Write the row shape PRD-0064 introduces: an account notebook with no
    /// published copy, so `notebook_json` and `pushed_at` are both `NONE` and
    /// the content lives in `draft_json`.
    async fn write_unpublished_row(db: &Db, id: &str) -> Result<()> {
        db.inner
            .query(
                "CREATE type::record('mutable_share', $id) SET \
                    notebook_json = NONE, manifest_json = NONE, \
                    draft_json = $draft, draft_bytes = 7, bytes = 0, \
                    pushed_at = NONE, created_at = $now",
            )
            .bind(("id", id.to_string()))
            .bind(("draft", "{\"d\":1}".to_string()))
            .bind(("now", now_rfc3339()))
            .await?
            .check()?;
        Ok(())
    }

    #[derive(SurrealValue)]
    struct WidenedRow {
        notebook_json: Option<String>,
        draft_json: Option<String>,
        pushed_at: Option<String>,
    }

    async fn read_widened(db: &Db, id: &str) -> Option<WidenedRow> {
        db.inner
            .query(
                "SELECT notebook_json, draft_json, pushed_at \
                 FROM ONLY type::record('mutable_share', $id)",
            )
            .bind(("id", id.to_string()))
            .await
            .unwrap()
            .take(0)
            .unwrap()
    }

    #[tokio::test]
    async fn overwrite_widens_notebook_json_on_a_database_that_already_has_rows() {
        let dir = tempfile::tempdir().unwrap();
        let db = pre_0064_db_with_a_published_share(&dir).await;
        let before = read_widened(&db, PRE_0064_ID)
            .await
            .expect("the fixture must have written a row under the old schema");

        // The REAL boot-time schema pass, against a database that predates
        // the widening — which is the only shape of this test that can fail.
        db.define_schema()
            .await
            .expect("define_schema must apply to a table that already holds rows");

        // The pre-existing published row survives the redefinition, and the
        // read path that still types these fields as `String` still works.
        let row = db
            .get_mutable_share(PRE_0064_ID)
            .await
            .unwrap()
            .expect("the row written under the old schema must still read back");
        assert_eq!(row.notebook_json.as_deref(), Some("{\"v\":1}"));
        assert_eq!(row.manifest_json.as_deref(), Some("{\"m\":1}"));
        assert_eq!(row.owner.map(|o| o.login), Some("author".to_string()));
        assert_eq!(
            read_widened(&db, PRE_0064_ID).await.unwrap().pushed_at,
            before.pushed_at,
            "pushed_at not clobbered"
        );

        // ...and the new shape is now accepted.
        write_unpublished_row(&db, "bbbbbbbbbbbbbbbb")
            .await
            .expect("a NONE notebook_json must be accepted after the widening");
        let row = read_widened(&db, "bbbbbbbbbbbbbbbb").await.unwrap();
        assert_eq!(row.notebook_json, None);
        assert_eq!(row.pushed_at, None);
        assert_eq!(row.draft_json.as_deref(), Some("{\"d\":1}"));

        // `define_schema` runs on every boot, so the OVERWRITE has to be
        // harmless the second time, with both row shapes already present.
        db.define_schema()
            .await
            .expect("the OVERWRITE must be re-runnable: it runs on every boot");
        assert_eq!(
            db.get_mutable_share(PRE_0064_ID)
                .await
                .unwrap()
                .unwrap()
                .notebook_json
                .as_deref(),
            Some("{\"v\":1}")
        );
        assert_eq!(
            read_widened(&db, "bbbbbbbbbbbbbbbb")
                .await
                .unwrap()
                .notebook_json,
            None
        );
    }

    #[tokio::test]
    async fn if_not_exists_declines_to_widen_an_existing_field() {
        // Guard the guard. If this arm ever accepts the NONE write, the trap
        // PRD-0064 is built around does not exist and `IF NOT EXISTS` would
        // have been fine.
        let dir = tempfile::tempdir().unwrap();
        let db = pre_0064_db_with_a_published_share(&dir).await;

        run_sql(&db, WIDEN_WITH_IF_NOT_EXISTS)
            .await
            .expect("IF NOT EXISTS is not an error against an existing field, just a no-op");

        let err = write_unpublished_row(&db, "bbbbbbbbbbbbbbbb")
            .await
            .expect_err("IF NOT EXISTS must leave the old TYPE string in force");
        let msg = err.to_string();
        assert!(
            msg.contains("notebook_json"),
            "the rejection must name the field that is still `string`: {msg}"
        );
        assert!(
            read_widened(&db, "bbbbbbbbbbbbbbbb").await.is_none(),
            "the rejected write must not have landed"
        );
    }
}

#[cfg(test)]
mod concurrency_tests {
    use super::*;

    /// Concurrent writers to ONE record must all land.
    ///
    /// `SurrealKV` is optimistically concurrent, so the losers of a race get a
    /// conflict the engine itself marks retryable. Before `with_conflict_retry`
    /// this failed 7 of 8: two browser tabs autosaving one account notebook
    /// (PRD-0064 made the server draft the primary storage path) and one user
    /// signing in from two devices both take this shape.
    ///
    /// `multi_thread` is load-bearing. `#[tokio::test]` defaults to a
    /// current-thread runtime where these tasks interleave at await points
    /// instead of running in parallel, and the conflict never opens: this same
    /// test passed 8/8 against the UNFIXED code until the flavor was set.
    #[tokio::test(flavor = "multi_thread", worker_threads = 8)]
    async fn concurrent_writers_to_one_record_all_succeed() {
        const WRITERS: usize = 8;

        let dir = tempfile::tempdir().expect("tmp");
        let db = Db::open(&dir.path().join("c.db")).await.expect("open");
        db.upsert_user("1", "owner", "").await.expect("seed user");
        let share = db
            .create_account_notebook("1", r#"{"title":"t","cells":[]}"#)
            .await
            .expect("seed share");

        let mut set = tokio::task::JoinSet::new();
        for i in 0..WRITERS {
            let (db, share) = (db.clone(), share.clone());
            set.spawn(async move {
                db.save_draft(&share, &format!(r#"{{"title":"t{i}","cells":[]}}"#))
                    .await
            });
        }
        let mut errors = Vec::new();
        while let Some(res) = set.join_next().await {
            if let Err(e) = res.expect("join") {
                errors.push(format!("{e:#}"));
            }
        }
        assert!(
            errors.is_empty(),
            "{} of {WRITERS} concurrent draft saves failed: {errors:?}",
            errors.len()
        );

        // The record is still readable and holds one of the writes, rather
        // than a torn value from the losing attempts.
        let edit = db.get_share_for_edit(&share).await.expect("read back");
        assert!(edit.is_some(), "the contended share must survive the race");

        let mut set = tokio::task::JoinSet::new();
        for _ in 0..WRITERS {
            let db = db.clone();
            set.spawn(async move { db.upsert_user("1", "owner", "").await });
        }
        let mut errors = Vec::new();
        while let Some(res) = set.join_next().await {
            if let Err(e) = res.expect("join") {
                errors.push(format!("{e:#}"));
            }
        }
        assert!(
            errors.is_empty(),
            "{} of {WRITERS} concurrent sign-ins failed: {errors:?}",
            errors.len()
        );
    }
}
