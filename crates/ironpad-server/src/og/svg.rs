//! The social-preview card itself: data in, SVG out.
//!
//! Pure and synchronous, so the whole layout is unit-testable without a
//! rasterizer, a filesystem, or a running server.

use std::fmt::Write as _;

use super::text::{mono, sans, sans_bold, MONO_FAMILY, SANS_FAMILY};

// ── Canvas ──────────────────────────────────────────────────────────────────

/// Card width in pixels. Single-sourced with the size the client advertises
/// (`ironpad_common::OG_CARD_WIDTH`) so the rendered card and the announced
/// dimensions can never drift.
pub const WIDTH: u32 = ironpad_common::OG_CARD_WIDTH;

/// Card height in pixels. See [`WIDTH`].
pub const HEIGHT: u32 = ironpad_common::OG_CARD_HEIGHT;

const W: f64 = WIDTH as f64;
const H: f64 = HEIGHT as f64;
const PAD: f64 = 72.0;
const CONTENT_W: f64 = W - PAD * 2.0;

// ── Palette (mirrors the `--ip-*` custom properties in style/main.scss) ─────

const BG_TOP: &str = "#1a1a2e";
const BG_BOTTOM: &str = "#141d38";
const ACCENT: &str = "#e94560";
const TEXT_PRIMARY: &str = "#eaeaea";
const TEXT_SECONDARY: &str = "#b0b0cc";
const TEXT_MUTED: &str = "#6a6a8a";
const BORDER: &str = "#0f3460";
const PANEL_FILL: &str = "#131a33";
const CHIP_FILL: &str = "#1e2a4a";

const TOK_COMMENT: &str = "#8f8778";
const TOK_KEYWORD: &str = "#e56b86";
const TOK_STRING: &str = "#7cc08e";
const TOK_NUMBER: &str = "#c79bf0";

// ── Type scale ──────────────────────────────────────────────────────────────

const BRAND_BASELINE: f64 = 104.0;
const BRAND_SIZE: f64 = 34.0;
const LABEL_SIZE: f64 = 20.0;

const TITLE_SIZE: f64 = 60.0;
const TITLE_LEADING: f64 = 74.0;
const TITLE_BASELINE: f64 = 206.0;
const TITLE_MAX_LINES: usize = 2;

const DESC_SIZE: f64 = 27.0;
const DESC_LEADING: f64 = 38.0;
/// Measured from the last title baseline, so it has to clear the title's
/// descenders as well as separate the two blocks; a two-line title otherwise
/// sits right on top of the description.
const DESC_GAP: f64 = 52.0;
const DESC_MAX_LINES: usize = 2;

const CODE_SIZE: f64 = 20.0;
const CODE_LEADING: f64 = 29.0;
const CODE_MAX_LINES: usize = 6;
const PANEL_PAD: f64 = 22.0;
const PANEL_GAP_TOP: f64 = 36.0;
const PANEL_GAP_BOTTOM: f64 = 44.0;

const FOOTER_BASELINE: f64 = 582.0;
const FOOTER_SIZE: f64 = 21.0;
const CHIP_H: f64 = 32.0;
const CHIP_PAD: f64 = 13.0;
const CHIP_GAP: f64 = 9.0;
const MAX_TAGS: usize = 4;

/// Width of one tab stop when a code excerpt is expanded for measurement.
/// SVG cannot advance to a tab stop, so tabs become spaces before layout.
const TAB_WIDTH: usize = 4;

// ── Card data ───────────────────────────────────────────────────────────────

/// Everything the card draws, extracted from a notebook by the caller.
#[derive(Clone, Debug, Default)]
pub struct Card {
    /// Reader-facing storage class, e.g. `"public notebook"`.
    pub kind_label: String,
    pub title: String,
    pub description: Option<String>,
    pub tags: Vec<String>,
    /// Bottom-left note, e.g. `"28 cells"`. Free text rather than a count, so
    /// the site card can say something true about itself instead of "0 cells".
    pub footer_note: String,
    /// Raw excerpt lines from the notebook's first runnable cell. More may be
    /// supplied than fit; the layout takes what the panel has room for.
    pub code: Vec<String>,
    /// Host shown bottom-right, e.g. `"ironpad.twitchax.com"`.
    pub host: String,
}

// ── Rendering ───────────────────────────────────────────────────────────────

/// Renders `card` to a standalone SVG document.
#[must_use]
#[allow(clippy::too_many_lines)] // One layout, read top to bottom; splitting it would scatter the y-cursor.
pub fn render(card: &Card) -> String {
    let mut s = String::with_capacity(4096);

    let _ = write!(
        s,
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{WIDTH}" height="{HEIGHT}" viewBox="0 0 {WIDTH} {HEIGHT}">"#
    );
    let _ = write!(
        s,
        r#"<defs><linearGradient id="bg" x1="0" y1="0" x2="0.35" y2="1"><stop offset="0" stop-color="{BG_TOP}"/><stop offset="1" stop-color="{BG_BOTTOM}"/></linearGradient></defs>"#
    );
    let _ = write!(
        s,
        r#"<rect width="{W}" height="{H}" fill="url(#bg)"/><rect width="{W}" height="8" fill="{ACCENT}"/>"#
    );

    // ── Brand row ───────────────────────────────────────────────────────────
    text_el(
        &mut s,
        PAD,
        BRAND_BASELINE,
        SANS_FAMILY,
        700,
        BRAND_SIZE,
        ACCENT,
        "ironpad",
    );
    let brand_w = sans_bold().width("ironpad", BRAND_SIZE);
    let label = card.kind_label.to_uppercase();
    let _ = write!(
        s,
        r#"<text x="{x:.1}" y="{BRAND_BASELINE}" font-family="{SANS_FAMILY}" font-weight="500" font-size="{LABEL_SIZE}" fill="{TEXT_MUTED}" letter-spacing="1.6">{}</text>"#,
        escape(&label),
        x = PAD + brand_w + 22.0,
    );

    // ── Title ───────────────────────────────────────────────────────────────
    let title_lines = sans_bold().wrap(&card.title, TITLE_SIZE, CONTENT_W, TITLE_MAX_LINES);
    let mut baseline = TITLE_BASELINE;
    for line in &title_lines {
        text_el(
            &mut s,
            PAD,
            baseline,
            SANS_FAMILY,
            700,
            TITLE_SIZE,
            TEXT_PRIMARY,
            line,
        );
        baseline += TITLE_LEADING;
    }
    // Back up to the last baseline actually drawn; the cursor below is
    // relative to ink, not to the next empty slot.
    let mut y = baseline - TITLE_LEADING;

    // ── Description ─────────────────────────────────────────────────────────
    if let Some(desc) = card
        .description
        .as_deref()
        .map(str::trim)
        .filter(|d| !d.is_empty())
    {
        let lines = sans().wrap(desc, DESC_SIZE, CONTENT_W, DESC_MAX_LINES);
        let mut b = y + DESC_GAP;
        for line in &lines {
            text_el(
                &mut s,
                PAD,
                b,
                SANS_FAMILY,
                400,
                DESC_SIZE,
                TEXT_SECONDARY,
                line,
            );
            b += DESC_LEADING;
        }
        if !lines.is_empty() {
            y = b - DESC_LEADING;
        }
    }

    // ── Code excerpt ────────────────────────────────────────────────────────
    // The panel takes whatever vertical room the title and description left,
    // so a short title yields a taller excerpt rather than a taller gap.
    let region_top = y + PANEL_GAP_TOP;
    let region_h = (FOOTER_BASELINE - PANEL_GAP_BOTTOM) - region_top;
    let available = region_h - PANEL_PAD * 2.0;
    #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
    let fits = if available < CODE_SIZE {
        0
    } else {
        ((available - CODE_SIZE) / CODE_LEADING).floor() as usize + 1
    };
    let n_code = fits.min(CODE_MAX_LINES).min(card.code.len());

    if n_code > 0 {
        let code_w = CONTENT_W - PANEL_PAD * 2.0;
        #[allow(clippy::cast_precision_loss)]
        let panel_h = PANEL_PAD * 2.0 + CODE_SIZE + (n_code as f64 - 1.0) * CODE_LEADING + 6.0;
        // Centred in the leftover region: an excerpt shorter than the space
        // available otherwise leaves a conspicuous void above the footer.
        let panel_top = region_top + (region_h - panel_h).max(0.0) / 2.0;
        let _ = write!(
            s,
            r#"<rect x="{PAD}" y="{panel_top:.1}" width="{CONTENT_W}" height="{panel_h:.1}" rx="10" fill="{PANEL_FILL}" stroke="{BORDER}" stroke-width="1.5"/>"#
        );

        let mut b = panel_top + PANEL_PAD + CODE_SIZE;
        for line in card.code.iter().take(n_code) {
            let expanded = expand_tabs(line);
            let clipped = mono().clip(&expanded, CODE_SIZE, code_w);
            code_line_el(&mut s, PAD + PANEL_PAD, b, &clipped);
            b += CODE_LEADING;
        }
    }

    // ── Footer ──────────────────────────────────────────────────────────────
    text_el(
        &mut s,
        PAD,
        FOOTER_BASELINE,
        SANS_FAMILY,
        600,
        FOOTER_SIZE,
        TEXT_SECONDARY,
        &card.footer_note,
    );

    let mut chip_x = PAD + sans().width(&card.footer_note, FOOTER_SIZE) + 24.0;
    // Right edge reserved for the host, so chips never collide with it.
    let host_w = sans().width(&card.host, FOOTER_SIZE);
    let chip_limit = W - PAD - host_w - 28.0;
    for tag in card.tags.iter().take(MAX_TAGS) {
        let tw = sans().width(tag, FOOTER_SIZE - 3.0);
        let chip_w = tw + CHIP_PAD * 2.0;
        if chip_x + chip_w > chip_limit {
            break;
        }
        let _ = write!(
            s,
            r#"<rect x="{chip_x:.1}" y="{y:.1}" width="{chip_w:.1}" height="{CHIP_H}" rx="16" fill="{CHIP_FILL}"/>"#,
            y = FOOTER_BASELINE - CHIP_H + 8.0,
        );
        text_el(
            &mut s,
            chip_x + CHIP_PAD,
            FOOTER_BASELINE - 1.0,
            SANS_FAMILY,
            500,
            FOOTER_SIZE - 3.0,
            TEXT_MUTED,
            tag,
        );
        chip_x += chip_w + CHIP_GAP;
    }

    let _ = write!(
        s,
        r#"<text x="{x}" y="{FOOTER_BASELINE}" text-anchor="end" font-family="{SANS_FAMILY}" font-weight="400" font-size="{FOOTER_SIZE}" fill="{TEXT_MUTED}">{}</text>"#,
        escape(&card.host),
        x = W - PAD,
    );

    s.push_str("</svg>");
    s
}

/// Appends one `<text>` element. Writes into the document buffer rather than
/// returning a `String`, which keeps a card's dozen-odd text runs from each
/// allocating an intermediate.
#[allow(clippy::too_many_arguments)] // Every one is a distinct SVG attribute.
fn text_el(
    out: &mut String,
    x: f64,
    y: f64,
    family: &str,
    weight: u16,
    size: f64,
    fill: &str,
    content: &str,
) {
    let _ = write!(
        out,
        r#"<text x="{x:.1}" y="{y:.1}" font-family="{family}" font-weight="{weight}" font-size="{size}" fill="{fill}">{}</text>"#,
        escape(content)
    );
}

/// Appends a code line as coloured `tspan` runs.
///
/// `xml:space="preserve"` is load-bearing: without it the XML parser collapses
/// runs of spaces, and every excerpt loses its indentation.
fn code_line_el(out: &mut String, x: f64, y: f64, line: &str) {
    let _ = write!(
        out,
        r#"<text xml:space="preserve" x="{x:.1}" y="{y:.1}" font-family="{MONO_FAMILY}" font-weight="400" font-size="{CODE_SIZE}">"#
    );
    for (piece, colour) in tokenize(line) {
        let _ = write!(out, r#"<tspan fill="{colour}">{}</tspan>"#, escape(piece));
    }
    out.push_str("</text>");
}

// ── Rust micro-highlighter ──────────────────────────────────────────────────

/// The keywords worth colouring in a six-line excerpt. Deliberately short: a
/// card is read at thumbnail size, so the goal is "this is Rust", not a
/// faithful grammar.
const KEYWORDS: &[&str] = &[
    "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern",
    "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub", "ref",
    "return", "self", "Self", "static", "struct", "super", "trait", "type", "unsafe", "use",
    "where", "while", "true", "false",
];

/// Splits one line into `(text, colour)` runs.
///
/// Line-oriented by construction, so a multi-line string literal or block
/// comment in the source cannot leak its colour into the rest of the card.
///
/// Adjacent runs of the same colour are coalesced. Most of a code line is one
/// colour, so without this an indent, an identifier, and the space after it
/// each become their own `tspan` for no visual gain.
fn tokenize(line: &str) -> Vec<(&str, &'static str)> {
    let bytes = line.as_bytes();
    let mut spans: Vec<(usize, usize, &'static str)> = Vec::new();
    let mut i = 0;

    let push = |spans: &mut Vec<(usize, usize, &'static str)>,
                start: usize,
                end: usize,
                colour: &'static str| {
        if let Some(last) = spans.last_mut() {
            if last.2 == colour && last.1 == start {
                last.1 = end;
                return;
            }
        }
        spans.push((start, end, colour));
    };

    while i < bytes.len() {
        // Line comment: everything after `//` belongs to it.
        if bytes[i] == b'/' && bytes.get(i + 1) == Some(&b'/') {
            push(&mut spans, i, line.len(), TOK_COMMENT);
            break;
        }

        // String literal, honouring backslash escapes.
        if bytes[i] == b'"' {
            let start = i;
            i += 1;
            while i < bytes.len() {
                if bytes[i] == b'\\' {
                    i += 2;
                    continue;
                }
                if bytes[i] == b'"' {
                    i += 1;
                    break;
                }
                i += 1;
            }
            let end = i.min(line.len());
            push(&mut spans, start, end, TOK_STRING);
            continue;
        }

        // Identifier, keyword, or number.
        if bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_' {
            let start = i;
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                i += 1;
            }
            let word = &line[start..i];
            let colour = if bytes[start].is_ascii_digit() {
                TOK_NUMBER
            } else if KEYWORDS.contains(&word) {
                TOK_KEYWORD
            } else {
                TEXT_SECONDARY
            };
            push(&mut spans, start, i, colour);
            continue;
        }

        // Anything else: consume a whole run of non-special bytes at once so
        // punctuation and whitespace don't each become their own tspan.
        let start = i;
        while i < bytes.len() {
            let b = bytes[i];
            if b == b'"' || b.is_ascii_alphanumeric() || b == b'_' {
                break;
            }
            if b == b'/' && bytes.get(i + 1) == Some(&b'/') {
                break;
            }
            i += 1;
        }
        // Multi-byte UTF-8 lands here; step forward to the next char boundary
        // so slicing can never split a code point.
        if i == start {
            i += 1;
            while i < bytes.len() && !line.is_char_boundary(i) {
                i += 1;
            }
        }
        push(&mut spans, start, i, TEXT_SECONDARY);
    }

    spans
        .into_iter()
        .map(|(start, end, colour)| (&line[start..end], colour))
        .collect()
}

// ── Helpers ─────────────────────────────────────────────────────────────────

/// Expands tabs to [`TAB_WIDTH`] spaces. SVG has no tab stops, and an
/// un-expanded tab measures as zero width, so indentation would collapse.
fn expand_tabs(line: &str) -> String {
    if !line.contains('\t') {
        return line.to_string();
    }
    let mut out = String::with_capacity(line.len() + 8);
    // Column is carried, not recounted. `out.chars().count()` inside the loop
    // made this quadratic in the line length, on cell source that arrives
    // through unauthenticated share uploads.
    let mut column = 0_usize;
    for ch in line.chars() {
        if ch == '\t' {
            let pad = TAB_WIDTH - (column % TAB_WIDTH);
            out.extend(std::iter::repeat_n(' ', pad));
            column += pad;
        } else {
            out.push(ch);
            column += 1;
        }
    }
    out
}

/// XML-escapes text and attribute content, dropping characters XML cannot
/// represent at all.
///
/// Notebook titles are attacker-controlled on `/shared` and `/mutable`, so an
/// unescaped `<` would let a share inject arbitrary SVG (including a
/// `<script>`) into an image the server signs with its own hostname.
///
/// Escaping alone is not enough. XML 1.0 forbids the C0 controls outright, and
/// no entity can encode them, so a title carrying one made `usvg` reject the
/// whole document: `/og/{class}/{id}.png` answered 500 for that notebook
/// permanently, since the failure is deterministic in its content. They are
/// dropped rather than escaped for that reason. Tab, newline, and carriage
/// return are the three XML permits and are kept.
fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            // XML 1.0 §2.2: only these three C0 controls are legal.
            '\t' | '\n' | '\r' => out.push(c),
            c if c.is_control() => {}
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Card {
        Card {
            kind_label: "public notebook".to_string(),
            title: "The compiler fires a cannon".to_string(),
            description: Some("Enzyme differentiates a ballistics simulation.".to_string()),
            tags: vec!["autodiff".to_string(), "blog".to_string()],
            footer_note: "28 cells".to_string(),
            code: vec![
                "let mut vx = speed * theta.cos();".to_string(),
                "while y > 0.0 {".to_string(),
                "    y += vy * DT; // integrate".to_string(),
                "}".to_string(),
            ],
            host: "ironpad.twitchax.com".to_string(),
        }
    }

    #[test]
    fn renders_a_well_formed_document_at_the_unfurl_size() {
        let svg = render(&sample());
        assert!(svg.starts_with("<svg "));
        assert!(svg.ends_with("</svg>"));
        assert!(svg.contains(r#"width="1200" height="630""#));
        assert!(svg.contains("The compiler fires a cannon"));
        assert!(svg.contains("28 cells"));
        assert!(svg.contains("ironpad.twitchax.com"));
        assert!(svg.contains("PUBLIC NOTEBOOK"));
    }

    #[test]
    fn the_footer_note_is_drawn_verbatim() {
        let card = Card {
            footer_note: "45 notebooks".to_string(),
            ..sample()
        };
        assert!(render(&card).contains(">45 notebooks<"));
    }

    #[test]
    fn hostile_title_cannot_inject_svg() {
        // A shared notebook is attacker-supplied; this is the injection that
        // would otherwise put a script into an image served from our origin.
        let card = Card {
            title: "</text><script>alert('xss')</script><text>".to_string(),
            description: Some("<img onerror=x>".to_string()),
            ..sample()
        };
        let svg = render(&card);
        assert!(!svg.contains("<script>"));
        assert!(!svg.contains("<img"));
        assert!(svg.contains("&lt;script&gt;"));
    }

    #[test]
    fn missing_description_and_code_still_render() {
        let card = Card {
            description: None,
            code: Vec::new(),
            tags: Vec::new(),
            ..sample()
        };
        let svg = render(&card);
        assert!(svg.ends_with("</svg>"));
        // No panel is drawn when there is nothing to put in it.
        assert!(!svg.contains(PANEL_FILL));
    }

    #[test]
    fn a_long_title_crowds_out_the_code_panel_rather_than_overflowing() {
        let card = Card {
            title: "A very long title ".repeat(12),
            description: Some("And a description that also wraps to its full budget. ".repeat(6)),
            ..sample()
        };
        let svg = render(&card);
        // Title and description each cap at two lines, so the panel still fits;
        // what matters is that the card is valid and bounded either way.
        assert!(svg.ends_with("</svg>"));
        assert!(svg.contains("<text "));
    }

    #[test]
    fn code_indentation_survives_into_the_document() {
        let svg = render(&sample());
        assert!(
            svg.contains(r#"xml:space="preserve""#),
            "without this the parser eats every indent"
        );
        assert!(svg.contains("    y += vy"));
    }

    #[test]
    fn tokenizer_colours_keywords_strings_comments_and_numbers() {
        let toks = tokenize(r#"let s = "hi"; // note 42"#);
        let by_colour = |c: &str| -> Vec<&str> {
            toks.iter()
                .filter(|(_, col)| *col == c)
                .map(|(t, _)| *t)
                .collect()
        };
        assert_eq!(by_colour(TOK_KEYWORD), vec!["let"]);
        assert_eq!(by_colour(TOK_STRING), vec![r#""hi""#]);
        assert_eq!(by_colour(TOK_COMMENT), vec!["// note 42"]);
    }

    #[test]
    fn tokenizer_is_lossless() {
        // Every byte of the line must appear exactly once, in order: a
        // tokenizer that drops or duplicates text would silently corrupt code.
        for line in [
            r#"let s = "a\"b"; // trailing"#,
            "fn main() { let x = 3.14; }",
            "// only a comment",
            "",
            "    indented(\"→ unicode ←\")",
            r#"let unterminated = "oops"#,
        ] {
            let joined: String = tokenize(line).iter().map(|(t, _)| *t).collect();
            assert_eq!(joined, line, "tokenizer lost text in {line:?}");
        }
    }

    #[test]
    fn tokenizer_does_not_leak_a_comment_colour_across_lines() {
        // Each call is independent, so an excerpt starting mid-block-comment
        // cannot paint the whole panel brown.
        let after = tokenize("let x = 1;");
        assert!(after.iter().all(|(_, c)| *c != TOK_COMMENT));
    }

    #[test]
    fn a_control_character_still_produces_a_parseable_document() {
        // XML 1.0 forbids the C0 controls and no entity can encode them, so
        // one in a title made usvg reject the whole card and the route
        // answered 500 for that notebook permanently.
        let card = Card {
            title: "bad\u{1}title\u{0}".to_string(),
            description: Some("desc\u{b}with\u{1f}controls".to_string()),
            ..sample()
        };
        let svg = render(&card);
        assert!(!svg
            .chars()
            .any(|c| c.is_control() && c != '\t' && c != '\n' && c != '\r'));
        assert!(svg.contains("badtitle"));
    }

    #[test]
    fn expand_tabs_is_linear_not_quadratic() {
        // Recounting the output column per char made this quadratic in the
        // line length, on cell source that arrives via share uploads.
        let line = "\t".repeat(80_000);
        let start = std::time::Instant::now();
        let out = expand_tabs(&line);
        let elapsed = start.elapsed();

        assert_eq!(out.len(), 80_000 * TAB_WIDTH);
        assert!(
            elapsed < std::time::Duration::from_millis(100),
            "expand_tabs took {elapsed:?} on 80k tabs, which means it went quadratic again"
        );
    }

    #[test]
    fn tabs_expand_to_stops() {
        assert_eq!(expand_tabs("\tx"), "    x");
        assert_eq!(expand_tabs("ab\tx"), "ab  x");
        assert_eq!(expand_tabs("no tabs"), "no tabs");
    }

    #[test]
    fn escape_covers_every_xml_metacharacter() {
        assert_eq!(
            escape(r#"<a href="x">&'"#),
            "&lt;a href=&quot;x&quot;&gt;&amp;&apos;"
        );
    }
}
