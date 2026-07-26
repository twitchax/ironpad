//! Social-preview cards: `/og/{class}/{id}.png`.
//!
//! Reddit, X, Slack, and Discord all decide what a link looks like by
//! fetching the HTML and reading `og:image`. They do not run JavaScript and
//! they do not execute cells, so the picture has to be a real PNG the server
//! can produce from notebook *metadata* alone.
//!
//! The card is built as SVG (see [`svg`]) and rasterized with `resvg` against
//! fonts compiled into the binary (see [`text`]). Renders are cached on disk
//! keyed by the SVG itself, so identical inputs never rasterize twice and any
//! change to the layout, the palette, or the notebook invalidates for free.
//!
//! A notebook whose whole point is a picture can override the generated card
//! with `og_image` in its JSON; that path is resolved by the page that emits
//! the meta tags, not here.

pub mod svg;
mod text;

use std::path::{Path, PathBuf};

use axum::extract::{Path as AxumPath, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use ironpad_common::IronpadNotebook;

use crate::state::AppState;
use svg::Card;

/// Aggregate cap on the card cache. Each PNG is tens of kilobytes and the
/// working set is one card per notebook per release, so this is far above
/// normal use; it exists so that a flood of distinct shared notebooks cannot
/// fill the same volume the compile cache and share store live on.
const MAX_CACHE_BYTES: u64 = 64 * 1024 * 1024;

/// Lines of source pulled from the notebook. More than the panel can show, so
/// layout can pick after it knows how much room the title and description left.
const CODE_EXCERPT_LINES: usize = 8;

/// How long a crawler or CDN may reuse a card.
///
/// Not `immutable` like `/pkg/` and `/share-blobs/`: those URLs carry a
/// content hash, while a card URL is stable across edits by design. An hour
/// keeps a pushed mutable share from showing a stale preview all day while
/// still absorbing the burst of fetches a popular link produces.
const CACHE_CONTROL: &str = "public, max-age=3600";

// ── Storage class ───────────────────────────────────────────────────────────

/// Which storage class a card is being rendered for, mirroring the canonical
/// route prefixes (PRD-0048).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    Public,
    Shared,
    Mutable,
}

impl Class {
    /// Parses the `{class}` path segment.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "public" => Some(Self::Public),
            "shared" => Some(Self::Shared),
            "mutable" => Some(Self::Mutable),
            _ => None,
        }
    }

    /// The route prefix this class lives under, used to build `og:url`.
    #[must_use]
    pub fn route_prefix(self) -> &'static str {
        match self {
            Self::Public => "/public",
            Self::Shared => "/shared",
            Self::Mutable => "/mutable",
        }
    }

    /// Reader-facing label printed on the card.
    ///
    /// `Mutable` reads as "shared" deliberately: "mutable share" is ironpad's
    /// internal vocabulary, and someone seeing the card in a feed only needs
    /// to know it is somebody's notebook rather than a bundled one.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Public => "public notebook",
            Self::Shared | Self::Mutable => "shared notebook",
        }
    }
}

// ── Card construction ───────────────────────────────────────────────────────

/// `"1 cell"` / `"12 cells"`, and the same for any other counted noun.
fn count_note(n: usize, noun: &str) -> String {
    if n == 1 {
        format!("1 {noun}")
    } else {
        format!("{n} {noun}s")
    }
}

/// Builds the card for `notebook`.
#[must_use]
pub fn card_for(notebook: &IronpadNotebook, class: Class, host: &str) -> Card {
    Card {
        kind_label: class.label().to_string(),
        title: notebook.title.clone(),
        description: notebook.description.clone(),
        tags: notebook.tags.clone().unwrap_or_default(),
        footer_note: count_note(notebook.cells.len(), "cell"),
        code: code_excerpt(notebook),
        host: host.to_string(),
    }
}

/// The site-wide card, used for the home page and anywhere a specific
/// notebook isn't the subject. `notebooks` is the showcase count shown in the
/// footer, which is the one number about the site worth putting on a card.
#[must_use]
pub fn site_card(host: &str, notebooks: usize) -> Card {
    Card {
        kind_label: "interactive rust notebooks".to_string(),
        title: "ironpad".to_string(),
        description: Some(
            "Write Rust in a notebook. Cells compile to WebAssembly and run in your browser."
                .to_string(),
        ),
        tags: Vec::new(),
        footer_note: count_note(notebooks, "notebook"),
        code: vec![
            "let xs: Vec<f64> = (0..1000).map(|i| f64::from(i) / 1000.0).collect();".to_string(),
            "let ys: Vec<f64> = xs.iter().map(|x| (x * TAU).sin()).collect();".to_string(),
            String::new(),
            "Chart::line(&xs, &ys).title(\"one cell, no toolchain\")".to_string(),
        ],
        host: host.to_string(),
    }
}

/// Source lines from the notebook's first runnable cell.
///
/// Leading blank lines are dropped so the excerpt starts on ink; a cell that
/// opens with a comment banner would otherwise spend the whole panel on it.
fn code_excerpt(notebook: &IronpadNotebook) -> Vec<String> {
    let Some(cell) = notebook
        .cells
        .iter()
        .filter(|c| c.is_runnable())
        .min_by_key(|c| c.order)
    else {
        return Vec::new();
    };

    cell.source
        .lines()
        .skip_while(|l| l.trim().is_empty())
        .take(CODE_EXCERPT_LINES)
        .map(str::to_string)
        .collect()
}

// ── Rasterization ───────────────────────────────────────────────────────────

/// Rasterizes an SVG card to PNG bytes.
///
/// # Errors
///
/// Fails if the SVG does not parse or the pixmap cannot be allocated.
pub fn rasterize(document: &str) -> anyhow::Result<Vec<u8>> {
    use resvg::tiny_skia;
    use resvg::usvg;

    let options = usvg::Options {
        fontdb: text::database(),
        ..Default::default()
    };

    let tree = usvg::Tree::from_str(document, &options)
        .map_err(|e| anyhow::anyhow!("card SVG did not parse: {e}"))?;

    let mut pixmap = tiny_skia::Pixmap::new(svg::WIDTH, svg::HEIGHT).ok_or_else(|| {
        anyhow::anyhow!("could not allocate a {}x{} pixmap", svg::WIDTH, svg::HEIGHT)
    })?;

    resvg::render(
        &tree,
        tiny_skia::Transform::identity(),
        &mut pixmap.as_mut(),
    );

    pixmap
        .encode_png()
        .map_err(|e| anyhow::anyhow!("PNG encode failed: {e}"))
}

// ── Disk cache ──────────────────────────────────────────────────────────────

fn cache_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("og")
}

/// Cache key for a card.
///
/// Hashing the finished SVG rather than the notebook means every input that
/// can change a pixel is covered: the notebook's text, the layout constants,
/// and the palette. The release version is mixed in so a font swap or a
/// `resvg` bump, which the SVG cannot see, still invalidates.
fn cache_key(document: &str) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(env!("CARGO_PKG_VERSION").as_bytes());
    hasher.update(document.as_bytes());
    hasher.finalize().to_hex()[..32].to_string()
}

/// Sum of the sizes of regular files directly under `dir`; `0` when absent.
async fn dir_total_bytes(dir: &Path) -> u64 {
    let Ok(mut entries) = tokio::fs::read_dir(dir).await else {
        return 0;
    };
    let mut total = 0_u64;
    while let Ok(Some(entry)) = entries.next_entry().await {
        if let Ok(meta) = entry.metadata().await {
            if meta.is_file() {
                total = total.saturating_add(meta.len());
            }
        }
    }
    total
}

/// Renders `card`, serving a cached PNG when one exists.
///
/// A cache write that fails is logged and ignored: the bytes are already in
/// hand, and refusing to serve a preview because the disk is full would turn
/// a housekeeping problem into a visible one.
async fn render_cached(data_dir: &Path, card: &Card) -> anyhow::Result<Vec<u8>> {
    let document = svg::render(card);
    let dir = cache_dir(data_dir);
    let path = dir.join(format!("{}.png", cache_key(&document)));

    if let Ok(bytes) = tokio::fs::read(&path).await {
        return Ok(bytes);
    }

    let png = tokio::task::spawn_blocking(move || rasterize(&document))
        .await
        .map_err(|e| anyhow::anyhow!("card render task panicked: {e}"))??;

    if dir_total_bytes(&dir).await + png.len() as u64 <= MAX_CACHE_BYTES {
        if let Err(e) = write_atomic(&dir, &path, &png).await {
            tracing::warn!(error = %e, "could not cache social card");
        }
    } else {
        tracing::warn!("social card cache at capacity; serving without caching");
    }

    Ok(png)
}

/// Writes via a temporary file plus rename, so a concurrent reader never
/// observes a half-written PNG.
async fn write_atomic(dir: &Path, path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    tokio::fs::create_dir_all(dir).await?;
    let tmp = path.with_extension("png.tmp");
    tokio::fs::write(&tmp, bytes).await?;
    tokio::fs::rename(&tmp, path).await?;
    Ok(())
}

// ── Handlers ────────────────────────────────────────────────────────────────

/// Strips the `.png` suffix and rejects anything that could escape the
/// notebook directories. The `_core` loaders validate too; this refuses the
/// obviously-hostile shapes before any filesystem work happens.
fn notebook_id_from(file: &str) -> Option<&str> {
    let id = file.strip_suffix(".png")?;
    if id.is_empty() || id.contains('/') || id.contains('\\') || id.contains("..") {
        return None;
    }
    Some(id)
}

fn png_response(bytes: Vec<u8>) -> Response {
    (
        [
            (header::CONTENT_TYPE, "image/png"),
            (header::CACHE_CONTROL, CACHE_CONTROL),
        ],
        bytes,
    )
        .into_response()
}

fn host_of(public_url: &str) -> &str {
    public_url
        .trim_end_matches('/')
        .rsplit("://")
        .next()
        .unwrap_or(public_url)
}

/// `GET /og/{class}/{id}.png` — the card for one notebook.
pub async fn notebook_card_handler(
    State(state): State<AppState>,
    AxumPath((class, file)): AxumPath<(String, String)>,
) -> Response {
    let (Some(class), Some(id)) = (Class::parse(&class), notebook_id_from(&file)) else {
        return (StatusCode::NOT_FOUND, "not found").into_response();
    };

    let notebook = match class {
        Class::Public => {
            let site_root = Path::new(state.leptos_options.site_root.as_ref()).to_path_buf();
            ironpad_app::server_fns::get_public_notebook_core(&site_root, id).await
        }
        Class::Shared => {
            ironpad_app::server_fns::get_shared_notebook_core(&state.config.data_dir, id).await
        }
        // A mutable share that was unpublished resolves to `Ok(None)`, which
        // is a 404 here exactly like a hash that never existed.
        Class::Mutable => {
            match ironpad_app::server_fns::get_mutable_notebook_core(&state.config.data_dir, id)
                .await
            {
                Ok(Some(nb)) => Ok(nb),
                Ok(None) => Err(anyhow::anyhow!("no such mutable share")),
                Err(e) => Err(e),
            }
        }
    };

    let Ok(notebook) = notebook else {
        return (StatusCode::NOT_FOUND, "not found").into_response();
    };

    let card = card_for(&notebook, class, host_of(&state.config.public_url));
    match render_cached(&state.config.data_dir, &card).await {
        Ok(png) => png_response(png),
        Err(e) => {
            tracing::error!(error = %e, "social card render failed");
            (StatusCode::INTERNAL_SERVER_ERROR, "render failed").into_response()
        }
    }
}

/// `GET /og/ironpad.png` — the site-wide card, used by the home page.
pub async fn site_card_handler(State(state): State<AppState>) -> Response {
    let site_root = Path::new(state.leptos_options.site_root.as_ref()).to_path_buf();
    let notebooks = ironpad_app::server_fns::list_public_notebooks_core(&site_root)
        .await
        .map_or(0, |list| list.len());

    let card = site_card(host_of(&state.config.public_url), notebooks);
    match render_cached(&state.config.data_dir, &card).await {
        Ok(png) => png_response(png),
        Err(e) => {
            tracing::error!(error = %e, "site card render failed");
            (StatusCode::INTERNAL_SERVER_ERROR, "render failed").into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;

    use super::*;
    use ironpad_common::{CellType, IronpadCell};

    fn cell(order: u32, kind: CellType, shared: bool, source: &str) -> IronpadCell {
        IronpadCell {
            id: format!("cell_{order}"),
            order,
            label: format!("Cell {order}"),
            cell_type: kind,
            source: source.to_string(),
            cargo_toml: None,
            shared,
            collapsed: false,
            output_collapsed: false,
            version: 0,
        }
    }

    fn notebook(cells: Vec<IronpadCell>) -> IronpadNotebook {
        let mut nb = IronpadNotebook::new("Test notebook");
        nb.description = Some("A description.".to_string());
        nb.tags = Some(vec!["blog".to_string()]);
        nb.cells = cells;
        nb
    }

    #[test]
    fn class_parses_only_the_canonical_prefixes() {
        assert_eq!(Class::parse("public"), Some(Class::Public));
        assert_eq!(Class::parse("shared"), Some(Class::Shared));
        assert_eq!(Class::parse("mutable"), Some(Class::Mutable));
        assert_eq!(Class::parse("local"), None);
        assert_eq!(Class::parse(".."), None);
        assert_eq!(Class::parse(""), None);
    }

    #[test]
    fn notebook_id_rejects_traversal_and_requires_the_png_suffix() {
        assert_eq!(notebook_id_from("cannon.png"), Some("cannon"));
        assert_eq!(
            notebook_id_from("a1b2c3d4e5f60718.png"),
            Some("a1b2c3d4e5f60718")
        );
        assert_eq!(notebook_id_from("cannon"), None);
        assert_eq!(notebook_id_from(".png"), None);
        assert_eq!(notebook_id_from("../../etc/passwd.png"), None);
        assert_eq!(notebook_id_from("a/b.png"), None);
        assert_eq!(notebook_id_from("a\\b.png"), None);
    }

    #[test]
    fn excerpt_takes_the_first_runnable_cell_in_notebook_order() {
        let nb = notebook(vec![
            cell(0, CellType::Markdown, false, "# not code"),
            cell(1, CellType::Code, true, "// shared, never runs"),
            cell(2, CellType::Code, false, "let answer = 42;"),
            cell(3, CellType::Code, false, "let later = 1;"),
        ]);
        assert_eq!(code_excerpt(&nb), vec!["let answer = 42;"]);
    }

    #[test]
    fn excerpt_ignores_declaration_order_and_uses_the_order_field() {
        let nb = notebook(vec![
            cell(9, CellType::Code, false, "last"),
            cell(2, CellType::Code, false, "first"),
        ]);
        assert_eq!(code_excerpt(&nb), vec!["first"]);
    }

    #[test]
    fn excerpt_skips_leading_blank_lines_and_is_bounded() {
        let mut source = String::from("\n\n\n");
        for i in 1..=20 {
            let _ = writeln!(source, "line{i};");
        }
        let nb = notebook(vec![cell(0, CellType::Code, false, &source)]);
        let excerpt = code_excerpt(&nb);
        assert_eq!(excerpt.len(), CODE_EXCERPT_LINES);
        assert_eq!(excerpt[0], "line1;");
    }

    #[test]
    fn a_notebook_with_no_runnable_cell_yields_no_excerpt() {
        let nb = notebook(vec![cell(0, CellType::Markdown, false, "# prose only")]);
        assert!(code_excerpt(&nb).is_empty());
    }

    #[test]
    fn card_carries_the_notebook_metadata() {
        let nb = notebook(vec![cell(0, CellType::Code, false, "let x = 1;")]);
        let card = card_for(&nb, Class::Public, "ironpad.twitchax.com");
        assert_eq!(card.title, "Test notebook");
        assert_eq!(card.kind_label, "public notebook");
        assert_eq!(card.footer_note, "1 cell");
        assert_eq!(card.tags, vec!["blog"]);
        assert_eq!(card.host, "ironpad.twitchax.com");
    }

    #[test]
    fn host_strips_the_scheme_and_trailing_slash() {
        assert_eq!(
            host_of("https://ironpad.twitchax.com"),
            "ironpad.twitchax.com"
        );
        assert_eq!(
            host_of("https://ironpad.twitchax.com/"),
            "ironpad.twitchax.com"
        );
        assert_eq!(host_of("http://localhost:3111"), "localhost:3111");
    }

    #[test]
    fn cache_key_tracks_the_document_and_is_a_safe_filename() {
        let a = cache_key("<svg>a</svg>");
        let b = cache_key("<svg>b</svg>");
        assert_ne!(a, b);
        assert_eq!(a, cache_key("<svg>a</svg>"), "must be deterministic");
        assert_eq!(a.len(), 32);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn rasterizes_a_real_card_to_a_png() {
        let nb = notebook(vec![cell(
            0,
            CellType::Code,
            false,
            "let mut vx = speed * theta.cos();\nwhile y > 0.0 {\n    y += vy * DT; // step\n}",
        )]);
        let card = card_for(&nb, Class::Public, "ironpad.twitchax.com");
        let png = rasterize(&svg::render(&card)).expect("card should rasterize");

        // PNG magic, then the IHDR dimensions the unfurlers key off.
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        let width = u32::from_be_bytes(png[16..20].try_into().unwrap());
        let height = u32::from_be_bytes(png[20..24].try_into().unwrap());
        assert_eq!((width, height), (svg::WIDTH, svg::HEIGHT));
    }

    #[test]
    fn a_hostile_title_rasterizes_as_text_rather_than_markup() {
        let mut nb = notebook(vec![cell(0, CellType::Code, false, "let x = 1;")]);
        nb.title = "</text><script>alert(1)</script>".to_string();
        let card = card_for(&nb, Class::Shared, "ironpad.twitchax.com");
        assert!(rasterize(&svg::render(&card)).is_ok());
    }

    #[tokio::test]
    async fn cache_round_trips_and_reuses_the_rendered_bytes() {
        let dir = tempfile::TempDir::new().unwrap();
        let card = site_card("ironpad.twitchax.com", 45);

        let first = render_cached(dir.path(), &card).await.unwrap();
        let path = cache_dir(dir.path()).join(format!("{}.png", cache_key(&svg::render(&card))));
        assert!(
            path.exists(),
            "first render should have populated the cache"
        );

        let second = render_cached(dir.path(), &card).await.unwrap();
        assert_eq!(first, second);

        // No stray temp files survive an atomic write.
        let mut names: Vec<String> = std::fs::read_dir(cache_dir(dir.path()))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
            .collect();
        names.sort();
        assert!(
            names.iter().all(|n| std::path::Path::new(n)
                .extension()
                .is_some_and(|e| e == "png")),
            "{names:?}"
        );
    }

    #[tokio::test]
    async fn a_full_cache_still_serves_the_card() {
        let dir = tempfile::TempDir::new().unwrap();
        let og = cache_dir(dir.path());
        std::fs::create_dir_all(&og).unwrap();
        std::fs::write(
            og.join("ballast.png"),
            vec![0_u8; usize::try_from(MAX_CACHE_BYTES).unwrap() + 1],
        )
        .unwrap();

        let card = site_card("ironpad.twitchax.com", 45);
        let png = render_cached(dir.path(), &card).await.unwrap();
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");

        // Over capacity, so nothing new was written.
        let entries = std::fs::read_dir(&og).unwrap().count();
        assert_eq!(entries, 1);
    }
}
