//! The glyph-to-icon mapping table (PRD-0062) — one file, one entry per UI
//! role.
//!
//! Call sites name a ROLE (`icons::HISTORY`), never a vendor symbol
//! (`LuHistory`). That is the whole point of this module: a weight change, a
//! single icon swap, or a migration to an entirely different set is an edit
//! here rather than a sweep through fifty files, and the icon dependency
//! stays out of the rest of the crate.
//!
//! Each entry carries the Unicode glyph it replaced, so a reviewer can check
//! the mapping against the old UI without archaeology.

use super::icon::IconData;

// ── Cell transport + status ─────────────────────────────────────────────────

/// Run a cell. Replaces `▶` (the app's most-used affordance, 14 sites).
pub const RUN: IconData = icondata_lu::LuPlay;
/// Run every cell. Replaces the double triangle `▶▶`, which carried a
/// distinct meaning from the single-cell run and keeps it here.
pub const RUN_ALL: IconData = icondata_lu::LuFastForward;
/// Cancel an in-flight run. Replaces `⏹`.
pub const STOP: IconData = icondata_lu::LuSquare;
/// Pause an animation. Replaces `⏸`.
pub const PAUSE: IconData = icondata_lu::LuPause;
/// Step one frame. Replaces `⏭`.
pub const STEP: IconData = icondata_lu::LuSkipForward;
/// Idle status, and the unsaved-source dot. Replaces `●`.
pub const IDLE: IconData = icondata_lu::LuCircle;
/// Queued for execution. Replaces `◎`.
pub const QUEUED: IconData = icondata_lu::LuCircleDot;
/// Compiling / running. Replaces `◐`.
pub const BUSY: IconData = icondata_lu::LuLoaderCircle;
/// Succeeded. Replaces `✓`.
pub const SUCCESS: IconData = icondata_lu::LuCheck;
/// Failed. Replaces `✕` in the status badge.
pub const ERROR: IconData = icondata_lu::LuX;
/// Dropped because a dependency failed. Replaces both `⛔` (status badge)
/// and `⊘` (the viewer's blocked notice) — one meaning, one icon.
pub const BLOCKED: IconData = icondata_lu::LuBan;
/// Awaiting reactive re-execution. Replaces `⏳`.
pub const PENDING: IconData = icondata_lu::LuHourglass;
/// Re-run a cell. Replaces `↻`.
pub const RERUN: IconData = icondata_lu::LuRotateCw;
/// Reactive mode. Replaces `⚡`.
pub const REACTIVE: IconData = icondata_lu::LuZap;
/// Agent collaboration session. The session button was the one toolbar
/// control still carrying a bare text label.
pub const SESSION: IconData = icondata_lu::LuUsers;

// ── Cell authoring ──────────────────────────────────────────────────────────

/// Cell menu ("⋯"): a real affordance, not punctuation.
pub const MORE: IconData = icondata_lu::LuEllipsis;
/// Per-cell settings / Cargo.toml. Replaces `⚙`.
pub const SETTINGS: IconData = icondata_lu::LuSettings;
/// Shared-cell marker. Replaces `⬡` — Lucide keeps the hexagon language.
pub const SHARED: IconData = icondata_lu::LuHexagon;
/// Linux-cell marker (PRD-0066): a cell that is a whole program run as a real
/// Linux process under an in-browser kernel. Deliberately NOT a terminal —
/// [`OUTPUT_PANEL`] already holds `LuTerminal`, and a second terminal here
/// would read as "this cell's output" rather than "this cell is a machine".
/// A container is what the cell actually is: an isolated Linux box with a
/// filesystem, subprocesses and threads. Replaces a bare text badge; the
/// shared-cell badge beside it is the shape this mirrors.
pub const LINUX: IconData = icondata_lu::LuContainer;
/// "Code loads collapsed" default toggle. Replaces the abstract `▤`.
pub const CODE_PANEL: IconData = icondata_lu::LuCode;
/// "Output loads collapsed" default toggle. Replaces the abstract `⊡`.
pub const OUTPUT_PANEL: IconData = icondata_lu::LuTerminal;
/// Drag handle. Replaces `⠿` (a braille-pattern texture).
pub const DRAG: IconData = icondata_lu::LuGripVertical;
/// Duplicate a cell, and the copy-to-clipboard button. Replaces `⧉`.
pub const COPY: IconData = icondata_lu::LuCopy;
/// Add a cell or a notebook. Replaces the ASCII `+` in "+ Code",
/// "+ Markdown", and "+ New Notebook" — a character doing an icon's job,
/// which the first sweep never looked for because it scanned for SYMBOLS
/// rather than for affordances. (The `⊞` this role originally claimed to
/// replace was "⊞ Export HTML"; that one is [`EXPORT`].)
pub const ADD: IconData = icondata_lu::LuSquarePlus;
/// Remove / delete. Replaces `⊗`.
pub const REMOVE: IconData = icondata_lu::LuCircleX;
/// Reorder. Replaces `⇅`.
pub const REORDER: IconData = icondata_lu::LuArrowUpDown;
/// Permanently delete a notebook or cell. Replaces `╳`, which is a
/// box-drawing character and so slipped past the first `glyph-check` sweep.
/// Distinct from [`REMOVE`]: this one destroys the thing.
pub const DELETE: IconData = icondata_lu::LuTrash2;

// ── Disclosure + navigation ─────────────────────────────────────────────────

/// Collapsed disclosure chevron. Replaces `▸`.
pub const CHEVRON_RIGHT: IconData = icondata_lu::LuChevronRight;
/// Expanded disclosure chevron. Replaces `▾`.
pub const CHEVRON_DOWN: IconData = icondata_lu::LuChevronDown;
/// Hamburger / notebook menu. Replaces `☰`.
pub const MENU: IconData = icondata_lu::LuMenu;
/// Close / dismiss, and grant revocation. Replaces `✕`.
pub const CLOSE: IconData = icondata_lu::LuX;
/// Opens off-site. Replaces `↗`.
pub const EXTERNAL: IconData = icondata_lu::LuExternalLink;
/// Fork-to-local, and nested/derived items. Replaces `↳`.
pub const FORK: IconData = icondata_lu::LuCornerDownRight;
/// Sort ascending. Replaces `↑`.
pub const SORT_UP: IconData = icondata_lu::LuArrowUp;
/// Sort descending. Replaces `↓`.
pub const SORT_DOWN: IconData = icondata_lu::LuArrowDown;
/// View (read-only) mode. Replaces `◉`.
pub const VIEW: IconData = icondata_lu::LuEye;
/// Edit mode. Replaces `✎`.
pub const EDIT: IconData = icondata_lu::LuPencil;

// ── Notebook classes + sharing ──────────────────────────────────────────────

/// Private (IndexedDB-backed) notebooks. Replaces the filled `◆`; the filled
/// reading is preserved by the `--filled` class rather than a second icon.
pub const PRIVATE: IconData = icondata_lu::LuDiamond;
/// Public (bundled showcase) notebooks. Replaces the outline `◇`.
pub const PUBLIC: IconData = icondata_lu::LuDiamond;
/// A private share the viewer may not read. Replaces `🔒`.
pub const LOCKED: IconData = icondata_lu::LuLock;
/// Promote draft to published. Replaces `⬆`.
pub const PUSH: IconData = icondata_lu::LuUpload;
/// Import a notebook from disk. Replaces `↑` on the home page (distinct
/// from the cell-list `↑ Move Up`, which is a sort direction).
pub const IMPORT: IconData = icondata_lu::LuImport;
/// Download a notebook. Replaces `↓` in the notebook menu.
pub const DOWNLOAD: IconData = icondata_lu::LuDownload;
/// Share (immutable). Replaces `↗` in the notebook menu.
pub const SHARE: IconData = icondata_lu::LuShare;
/// Export to HTML. Replaces `⊞`.
pub const EXPORT: IconData = icondata_lu::LuFileDown;
/// A mutable/published notebook. Replaces `⟳` on the home page badges.
pub const PUBLISHED: IconData = icondata_lu::LuGlobe;
/// Server-stored notebooks owned by the signed-in user (PRD-0064), for the
/// home page's Account filter chip. Deliberately NOT [`PUBLISHED`]: that
/// group holds published and unpublished notebooks at once, so a globe
/// would contradict every card inside it wearing [`LOCKED`]. Names where
/// the notebook LIVES, which is the axis the chips sort on.
pub const ACCOUNT: IconData = icondata_lu::LuCloud;
/// Version history. Replaces `🕘` — the glyph that started this migration.
pub const HISTORY: IconData = icondata_lu::LuHistory;
/// Restore a snapshot. Replaces `⎌`.
pub const RESTORE: IconData = icondata_lu::LuUndo2;
/// Serialized (saved) output badge. Replaces `◫`.
pub const SAVED_OUTPUT: IconData = icondata_lu::LuSave;
/// Notebook metadata / link-preview section. Replaces the `🗂` emoji that
/// hid in escape form (`\u{1f5c2}`) and so survived the first sweep.
pub const METADATA: IconData = icondata_lu::LuTags;

// ── Feedback ────────────────────────────────────────────────────────────────

/// Warning, and the error-boundary icon. Replaces `⚠` and `△`.
pub const WARNING: IconData = icondata_lu::LuTriangleAlert;
/// A checked box in exported HTML. Replaces `☑`.
pub const CHECKBOX_CHECKED: IconData = icondata_lu::LuSquareCheck;
/// An unchecked box in exported HTML. Replaces `☐`.
pub const CHECKBOX: IconData = icondata_lu::LuSquare;
/// Light theme. Replaces `☼`.
pub const THEME_LIGHT: IconData = icondata_lu::LuSun;
/// Dark theme. Replaces `☾`.
pub const THEME_DARK: IconData = icondata_lu::LuMoon;

/// Every mapped role, for the exhaustive render test. Kept adjacent to the
/// table so a new entry that forgets to land here is obvious in review.
#[cfg(test)]
pub(crate) const ALL: &[(&str, IconData)] = &[
    ("RUN", RUN),
    ("RUN_ALL", RUN_ALL),
    ("IMPORT", IMPORT),
    ("DOWNLOAD", DOWNLOAD),
    ("SHARE", SHARE),
    ("EXPORT", EXPORT),
    ("PUBLISHED", PUBLISHED),
    ("ACCOUNT", ACCOUNT),
    ("STOP", STOP),
    ("PAUSE", PAUSE),
    ("STEP", STEP),
    ("IDLE", IDLE),
    ("QUEUED", QUEUED),
    ("BUSY", BUSY),
    ("SUCCESS", SUCCESS),
    ("ERROR", ERROR),
    ("BLOCKED", BLOCKED),
    ("PENDING", PENDING),
    ("RERUN", RERUN),
    ("REACTIVE", REACTIVE),
    ("SESSION", SESSION),
    ("MORE", MORE),
    ("SETTINGS", SETTINGS),
    ("SHARED", SHARED),
    ("LINUX", LINUX),
    ("CODE_PANEL", CODE_PANEL),
    ("OUTPUT_PANEL", OUTPUT_PANEL),
    ("DRAG", DRAG),
    ("COPY", COPY),
    ("ADD", ADD),
    ("REMOVE", REMOVE),
    ("REORDER", REORDER),
    ("DELETE", DELETE),
    ("CHEVRON_RIGHT", CHEVRON_RIGHT),
    ("CHEVRON_DOWN", CHEVRON_DOWN),
    ("MENU", MENU),
    ("CLOSE", CLOSE),
    ("EXTERNAL", EXTERNAL),
    ("FORK", FORK),
    ("SORT_UP", SORT_UP),
    ("SORT_DOWN", SORT_DOWN),
    ("VIEW", VIEW),
    ("EDIT", EDIT),
    ("PRIVATE", PRIVATE),
    ("PUBLIC", PUBLIC),
    ("LOCKED", LOCKED),
    ("PUSH", PUSH),
    ("HISTORY", HISTORY),
    ("RESTORE", RESTORE),
    ("SAVED_OUTPUT", SAVED_OUTPUT),
    ("METADATA", METADATA),
    ("WARNING", WARNING),
    ("CHECKBOX_CHECKED", CHECKBOX_CHECKED),
    ("CHECKBOX", CHECKBOX),
    ("THEME_LIGHT", THEME_LIGHT),
    ("THEME_DARK", THEME_DARK),
];
