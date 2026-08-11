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

/// One mutable share row, with its owner attribution resolved.
#[derive(Debug, Clone)]
pub struct MutableShareRow {
    pub notebook_json: String,
    pub manifest_json: Option<String>,
    pub pushed_at: String,
    /// Only the owner and READ grantees may view (PRD-0061).
    pub private: bool,
    /// `None` only for a share whose grant row is missing (should not happen;
    /// creates are transactional).
    pub owner: Option<AuthUser>,
}

/// Summary row for the owner's "published" listing.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct OwnedShareRow {
    pub id: String,
    pub notebook_json: String,
    pub pushed_at: String,
}

/// The owner's editing view of a share (PRD-0054): the draft when one
/// exists, else the published copy, plus whether they differ.
#[derive(Debug, Clone)]
pub struct ShareEditRow {
    /// Draft content if a draft exists, otherwise the published content.
    pub notebook_json: String,
    /// True when a draft exists (draft may differ from published).
    pub dirty: bool,
    pub pushed_at: String,
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
    }

    /// Row counts for the admin overview (PRD-0063).
    ///
    /// One query per table rather than a join: they are unrelated tables and
    /// the panel wants a number from each, so a join would only make the
    /// failure modes harder to read.
    pub async fn instance_counts(&self) -> Result<(u64, u64, u64)> {
        async fn count(db: &Surreal<LocalDb>, table: &str) -> Result<u64> {
            let sql = format!("SELECT VALUE count() FROM {table} GROUP ALL");
            let mut res = db
                .query(sql)
                .await
                .with_context(|| format!("count of {table} failed"))?;
            let n: Option<u64> = res
                .take(0)
                .with_context(|| format!("count of {table} returned an error"))?;
            // An empty table yields no group row at all, which is 0 rather
            // than a failure.
            Ok(n.unwrap_or(0))
        }

        Ok((
            count(&self.inner, "user").await?,
            count(&self.inner, "session").await?,
            count(&self.inner, "mutable_share").await?,
        ))
    }

    // ── Sessions ────────────────────────────────────────────────────────

    /// Mint a session for a user, returning the plaintext token destined for
    /// the cookie. Only its hash is stored.
    #[tracing::instrument(name = "db_create_session", level = "info", skip_all)]
    pub async fn create_session(&self, github_id: &str) -> Result<String> {
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
        self.inner
            .query("DELETE type::record('session', $key)")
            .bind(("key", hash_token(token)))
            .await
            .context("session delete failed")?
            .check()
            .context("session delete returned an error")?;
        Ok(())
    }

    // ── Mutable shares + grants ─────────────────────────────────────────

    /// Create a share and its OWNER grant in one transaction. Fails if the id
    /// already exists (callers retry with a fresh id).
    #[tracing::instrument(name = "db_create_share", level = "info", skip_all, fields(id = %id))]
    pub async fn create_mutable_share(
        &self,
        id: &str,
        owner_github_id: &str,
        notebook_json: &str,
        manifest_json: Option<String>,
    ) -> Result<()> {
        let now = now_rfc3339();
        self.inner
            .query(
                "BEGIN;
                 CREATE type::record('mutable_share', $id) SET \
                    notebook_json = $nb, manifest_json = $mf, \
                    bytes = $bytes, pushed_at = $now, created_at = $now;
                 CREATE rbac_grant SET \
                    user = type::record('user', $uid), \
                    resource_kind = $kind, resource_id = $id, role = $role;
                 COMMIT;",
            )
            .bind(("id", id.to_string()))
            .bind(("nb", notebook_json.to_string()))
            .bind(("mf", manifest_json))
            .bind((
                "bytes",
                i64::try_from(notebook_json.len()).unwrap_or(i64::MAX),
            ))
            .bind(("now", now))
            .bind(("uid", owner_github_id.to_string()))
            .bind(("kind", KIND_MUTABLE_SHARE))
            .bind(("role", ROLE_OWNER))
            .await
            .context("share create failed")?
            .check()
            .context("share create returned an error")?;
        Ok(())
    }

    /// Whether a share id is taken (used when minting fresh ids).
    pub async fn mutable_share_exists(&self, id: &str) -> Result<bool> {
        #[derive(SurrealValue)]
        struct Row {
            pushed_at: String,
        }
        let row: Option<Row> = self
            .inner
            .query("SELECT pushed_at FROM ONLY type::record('mutable_share', $id)")
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
            notebook_json: String,
            manifest_json: Option<String>,
            pushed_at: String,
            private: Option<bool>,
        }

        let mut response = self
            .inner
            .query(
                "SELECT notebook_json, manifest_json, pushed_at, private \
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
            pushed_at: share.pushed_at,
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
        self.inner
            .query(
                "UPDATE type::record('mutable_share', $id) SET \
                    notebook_json = $nb, manifest_json = $mf, \
                    bytes = $bytes, pushed_at = $now",
            )
            .bind(("id", id.to_string()))
            .bind(("nb", notebook_json.to_string()))
            .bind(("mf", manifest_json))
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
    }

    /// The owner's editing view: the draft when one exists, else published.
    #[tracing::instrument(name = "db_get_share_for_edit", level = "info", skip_all, fields(id = %id))]
    pub async fn get_share_for_edit(&self, id: &str) -> Result<Option<ShareEditRow>> {
        #[derive(SurrealValue)]
        struct Row {
            notebook_json: String,
            draft_json: Option<String>,
            pushed_at: String,
            private: Option<bool>,
        }
        let row: Option<Row> = self
            .inner
            .query(
                "SELECT notebook_json, draft_json, pushed_at, private \
                 FROM ONLY type::record('mutable_share', $id)",
            )
            .bind(("id", id.to_string()))
            .await
            .context("edit fetch failed")?
            .take(0)
            .context("edit row malformed")?;
        Ok(row.map(|r| ShareEditRow {
            dirty: r.draft_json.is_some(),
            notebook_json: r.draft_json.unwrap_or(r.notebook_json),
            pushed_at: r.pushed_at,
            private: r.private.unwrap_or(false),
        }))
    }

    /// Clear the draft slot without promoting (Discard draft).
    #[tracing::instrument(name = "db_discard_draft", level = "info", skip_all, fields(id = %id))]
    pub async fn discard_draft(&self, id: &str) -> Result<()> {
        self.inner
            .query(
                "UPDATE type::record('mutable_share', $id) \
                 SET draft_json = NONE, draft_bytes = NONE",
            )
            .bind(("id", id.to_string()))
            .await
            .context("draft discard failed")?
            .check()
            .context("draft discard returned an error")?;
        Ok(())
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
            .bind(("mf", manifest_json))
            .bind(("bytes", i64::try_from(published_bytes).unwrap_or(i64::MAX)))
            .bind(("now", now_rfc3339()))
            .await
            .context("draft promote failed")?
            .check()
            .context("draft promote returned an error")?;
        Ok(())
    }

    /// Delete a share and every grant on it, in one transaction.
    #[tracing::instrument(name = "db_delete_share", level = "info", skip_all, fields(id = %id))]
    pub async fn delete_mutable_share(&self, id: &str) -> Result<()> {
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
        self.inner
            .query("UPDATE type::record('mutable_share', $id) SET private = $private")
            .bind(("id", id.to_string()))
            .bind(("private", private))
            .await
            .context("privacy update failed")?
            .check()
            .context("privacy update returned an error")?;
        Ok(())
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
    }

    /// Remove a READ grant (never the OWNER's).
    #[tracing::instrument(name = "db_revoke_read", level = "info", skip_all, fields(id = %id))]
    pub async fn revoke_read(&self, id: &str, github_id: &str) -> Result<()> {
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

    /// All shares a user owns, newest push first.
    #[tracing::instrument(name = "db_list_owned_shares", level = "info", skip_all)]
    pub async fn list_shares_owned_by(&self, github_id: &str) -> Result<Vec<OwnedShareRow>> {
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

        // N+1 by design: the owned-share count is tiny (single-digit), and the
        // per-id fetch avoids gymnastics joining across the JSON payload.
        let mut out = Vec::with_capacity(grants.len());
        for grant in grants {
            if let Some(share) = self.get_mutable_share(&grant.resource_id).await? {
                out.push(OwnedShareRow {
                    id: grant.resource_id,
                    notebook_json: share.notebook_json,
                    pushed_at: share.pushed_at,
                });
            }
        }
        out.sort_by(|a, b| b.pushed_at.cmp(&a.pushed_at));
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
}

// ── Helpers ─────────────────────────────────────────────────────────────────

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

        db.create_mutable_share("abcd1234abcd1234", "1", "{\"title\":\"nb\"}", None)
            .await
            .unwrap();

        assert!(db.mutable_share_exists("abcd1234abcd1234").await.unwrap());
        assert!(!db.mutable_share_exists("ffffffffffffffff").await.unwrap());

        // Owner attribution is resolved with the share.
        let row = db
            .get_mutable_share("abcd1234abcd1234")
            .await
            .unwrap()
            .expect("share exists");
        assert_eq!(row.notebook_json, "{\"title\":\"nb\"}");
        assert!(row.manifest_json.is_none());
        let owner = row.owner.expect("owner grant resolved");
        assert_eq!(owner.login, "author");
        assert_eq!(owner.github_id, "1");

        // RBAC: the author owns it; the rando does not.
        assert!(db.user_owns_share("1", "abcd1234abcd1234").await.unwrap());
        assert!(!db.user_owns_share("2", "abcd1234abcd1234").await.unwrap());

        // Push updates content + manifest + pushed_at.
        db.update_mutable_share(
            "abcd1234abcd1234",
            "{\"title\":\"nb2\"}",
            Some("{\"version\":1}".to_string()),
        )
        .await
        .unwrap();
        let row = db
            .get_mutable_share("abcd1234abcd1234")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.notebook_json, "{\"title\":\"nb2\"}");
        assert_eq!(row.manifest_json.as_deref(), Some("{\"version\":1}"));

        // Delete removes the share AND its grant.
        db.delete_mutable_share("abcd1234abcd1234").await.unwrap();
        assert!(db
            .get_mutable_share("abcd1234abcd1234")
            .await
            .unwrap()
            .is_none());
        assert!(!db.user_owns_share("1", "abcd1234abcd1234").await.unwrap());
    }

    #[tokio::test]
    async fn create_share_rejects_duplicate_id() {
        let (_dir, db) = test_db().await;
        db.upsert_user("1", "author", "https://a/a.png")
            .await
            .unwrap();
        db.create_mutable_share("aaaaaaaaaaaaaaaa", "1", "{}", None)
            .await
            .unwrap();
        assert!(
            db.create_mutable_share("aaaaaaaaaaaaaaaa", "1", "{}", None)
                .await
                .is_err(),
            "CREATE on an existing record id must fail"
        );
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

        db.create_mutable_share("aaaaaaaaaaaaaaaa", "1", "{\"a\":1}", None)
            .await
            .unwrap();
        db.create_mutable_share("bbbbbbbbbbbbbbbb", "1", "{\"b\":2}", None)
            .await
            .unwrap();
        db.create_mutable_share("cccccccccccccccc", "2", "{\"c\":3}", None)
            .await
            .unwrap();

        let mine = db.list_shares_owned_by("1").await.unwrap();
        assert_eq!(mine.len(), 2);
        assert!(mine.iter().all(|s| s.id != "cccccccccccccccc"));
        // Newest push first (b was created after a).
        assert!(mine[0].pushed_at >= mine[1].pushed_at);

        assert!(db.list_shares_owned_by("999").await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn total_mutable_bytes_sums_notebook_json_and_drafts() {
        let (_dir, db) = test_db().await;
        db.upsert_user("1", "author", "https://a/a.png")
            .await
            .unwrap();
        assert_eq!(db.total_mutable_bytes().await.unwrap(), 0);

        db.create_mutable_share("aaaaaaaaaaaaaaaa", "1", "12345", None)
            .await
            .unwrap();
        db.create_mutable_share("bbbbbbbbbbbbbbbb", "1", "1234567890", None)
            .await
            .unwrap();
        assert_eq!(db.total_mutable_bytes().await.unwrap(), 15);

        // A draft counts toward the total (it is owner-drivable storage)...
        db.save_draft("aaaaaaaaaaaaaaaa", "123456789012345678901234567890")
            .await
            .unwrap();
        assert_eq!(db.total_mutable_bytes().await.unwrap(), 45);
        // ...an overwrite replaces rather than accumulates...
        db.save_draft("aaaaaaaaaaaaaaaa", "12345678901234567890")
            .await
            .unwrap();
        assert_eq!(db.total_mutable_bytes().await.unwrap(), 35);
        // ...and both promote and discard return it to published-only.
        db.promote_draft("aaaaaaaaaaaaaaaa", None, 20)
            .await
            .unwrap();
        assert_eq!(db.total_mutable_bytes().await.unwrap(), 30);
        db.save_draft("bbbbbbbbbbbbbbbb", "xx").await.unwrap();
        db.discard_draft("bbbbbbbbbbbbbbbb").await.unwrap();
        assert_eq!(db.total_mutable_bytes().await.unwrap(), 30);

        // A multibyte draft counts BYTES, not chars: "日本語" is 3 chars but
        // 9 bytes. The old SurrealQL string::len returned 3, undercounting
        // the cap 3x for CJK/emoji drafts.
        db.save_draft("bbbbbbbbbbbbbbbb", "日本語").await.unwrap();
        assert_eq!(db.total_mutable_bytes().await.unwrap(), 30 + 9);
    }

    #[tokio::test]
    async fn draft_lifecycle_save_promote_discard() {
        let (_dir, db) = test_db().await;
        db.upsert_user("1", "author", "").await.unwrap();
        db.create_mutable_share("aaaaaaaaaaaaaaaa", "1", "{\"v\":1}", None)
            .await
            .unwrap();

        // Fresh share: clean, and the edit view serves the published copy.
        let edit = db
            .get_share_for_edit("aaaaaaaaaaaaaaaa")
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
        db.save_draft("aaaaaaaaaaaaaaaa", "{\"v\":2}")
            .await
            .unwrap();
        let edit = db
            .get_share_for_edit("aaaaaaaaaaaaaaaa")
            .await
            .unwrap()
            .unwrap();
        assert!(edit.dirty);
        assert_eq!(edit.notebook_json, "{\"v\":2}");
        let reader = db
            .get_mutable_share("aaaaaaaaaaaaaaaa")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(reader.notebook_json, "{\"v\":1}", "readers see published");

        // Promote: published := draft, manifest replaced, clean again.
        let before = reader.pushed_at.clone();
        db.promote_draft("aaaaaaaaaaaaaaaa", Some("{\"m\":1}".into()), 7)
            .await
            .unwrap();
        let reader = db
            .get_mutable_share("aaaaaaaaaaaaaaaa")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(reader.notebook_json, "{\"v\":2}");
        assert_eq!(reader.manifest_json.as_deref(), Some("{\"m\":1}"));
        assert!(reader.pushed_at >= before);
        assert!(
            !db.get_share_for_edit("aaaaaaaaaaaaaaaa")
                .await
                .unwrap()
                .unwrap()
                .dirty
        );

        // Promote with no draft: a no-op (published and manifest untouched).
        db.promote_draft("aaaaaaaaaaaaaaaa", None, 0).await.unwrap();
        let reader = db
            .get_mutable_share("aaaaaaaaaaaaaaaa")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(reader.notebook_json, "{\"v\":2}");
        assert_eq!(reader.manifest_json.as_deref(), Some("{\"m\":1}"));

        // Discard: the draft evaporates; published is untouched.
        db.save_draft("aaaaaaaaaaaaaaaa", "{\"v\":3}")
            .await
            .unwrap();
        db.discard_draft("aaaaaaaaaaaaaaaa").await.unwrap();
        let edit = db
            .get_share_for_edit("aaaaaaaaaaaaaaaa")
            .await
            .unwrap()
            .unwrap();
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
}
