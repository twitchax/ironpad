//! Embedded fonts and text metrics for the social-preview card.
//!
//! SVG has no text wrapping: every line is positioned explicitly, so the card
//! has to know how wide a string will render *before* it writes it out. That
//! is the entire reason this module exists.
//!
//! The fonts are compiled into the binary rather than discovered through
//! fontconfig. A card that silently falls back to a different face would
//! change layout without failing, and the failure would only be visible in
//! someone else's Reddit feed — the `rust:slim` runtime image ships no fonts
//! at all, so discovery would find nothing there while working fine on a dev
//! box. Embedding makes dev, CI, and prod render the same pixels.

use std::sync::{Arc, OnceLock};

use resvg::usvg::fontdb;

// ── Vendored faces ──────────────────────────────────────────────────────────

const SANS_DATA: &[u8] = include_bytes!("../../assets/fonts/Inter-Regular.ttf");
const SANS_BOLD_DATA: &[u8] = include_bytes!("../../assets/fonts/Inter-Bold.ttf");
const MONO_DATA: &[u8] = include_bytes!("../../assets/fonts/JetBrainsMono-Regular.ttf");

/// Family name the card's SVG asks for, matching the vendored sans files and
/// `--ip-font-sans` in the app stylesheet.
pub const SANS_FAMILY: &str = "Inter";

/// Family name for the code excerpt, matching the vendored mono file.
pub const MONO_FAMILY: &str = "JetBrains Mono";

/// The ellipsis appended when text is clipped. One glyph rather than three
/// periods, so it costs a third of the width it saves.
const ELLIPSIS: &str = "\u{2026}";

// ── Metrics ─────────────────────────────────────────────────────────────────

/// One font face, wrapped for advance-width measurement.
///
/// Widths are the plain sum of horizontal advances: no kerning, no shaping,
/// no ligatures. For Latin text in Inter the error is well under a percent,
/// and every consumer here clips to a column anyway, so the approximation can
/// only ever cost a few pixels of right-hand slack.
pub struct Font {
    face: ttf_parser::Face<'static>,
}

impl Font {
    fn parse(data: &'static [u8]) -> Self {
        Self {
            face: ttf_parser::Face::parse(data, 0).expect("vendored font is valid TrueType"),
        }
    }

    /// Advance of a single char, in font units.
    ///
    /// Missing glyphs fall back to `.notdef`'s advance, which is what will
    /// actually be drawn, so the measurement matches the render.
    fn advance_units(&self, c: char) -> f64 {
        let notdef = self
            .face
            .glyph_hor_advance(ttf_parser::GlyphId(0))
            .unwrap_or(0);

        f64::from(
            self.face
                .glyph_index(c)
                .and_then(|g| self.face.glyph_hor_advance(g))
                .unwrap_or(notdef),
        )
    }

    /// Pixels per font unit at `size`, or `None` for a degenerate face.
    fn scale(&self, size: f64) -> Option<f64> {
        let upem = f64::from(self.face.units_per_em());
        (upem != 0.0).then(|| size / upem)
    }

    /// Rendered width of `text` at `size` pixels.
    #[must_use]
    pub fn width(&self, text: &str, size: f64) -> f64 {
        let Some(scale) = self.scale(size) else {
            return 0.0;
        };
        let units: f64 = text.chars().map(|c| self.advance_units(c)).sum();
        units * scale
    }

    /// Greedy word wrap into at most `max_lines` lines of at most `max_width`
    /// pixels. Overflowing text is marked with an ellipsis on the last line,
    /// so a clipped title reads as clipped rather than as a complete thought.
    #[must_use]
    pub fn wrap(&self, text: &str, size: f64, max_width: f64, max_lines: usize) -> Vec<String> {
        if max_lines == 0 || max_width <= 0.0 {
            return Vec::new();
        }

        // Wrap everything, then clip. Titles and descriptions are short enough
        // that the wasted work is invisible, and it keeps the overflow case
        // out of the wrapping loop, where it was the only source of tangle.
        let mut lines = self.wrap_all(text, size, max_width);

        if lines.len() > max_lines {
            lines.truncate(max_lines);
            if let Some(last) = lines.last_mut() {
                *last = self.ellipsize(last, size, max_width);
            }
        }

        lines
    }

    fn wrap_all(&self, text: &str, size: f64, max_width: f64) -> Vec<String> {
        let space = self.width(" ", size);
        let mut lines = Vec::new();
        let mut line = String::new();
        let mut line_w = 0.0_f64;

        for word in text.split_whitespace() {
            let word_w = self.width(word, size);

            if line.is_empty() {
                // A single token wider than the column can never fit, so clip
                // it rather than let it bleed off the edge of the card.
                if word_w > max_width {
                    lines.push(self.ellipsize(word, size, max_width));
                } else {
                    line.push_str(word);
                    line_w = word_w;
                }
                continue;
            }

            if line_w + space + word_w <= max_width {
                line.push(' ');
                line.push_str(word);
                line_w += space + word_w;
                continue;
            }

            lines.push(std::mem::take(&mut line));
            line_w = 0.0;

            if word_w > max_width {
                lines.push(self.ellipsize(word, size, max_width));
            } else {
                line.push_str(word);
                line_w = word_w;
            }
        }

        if !line.is_empty() {
            lines.push(line);
        }

        lines
    }

    /// Clips `text` so that it plus a trailing ellipsis fits `max_width`.
    ///
    /// Measures **forward**, accumulating advances until the budget runs out.
    /// The obvious implementation (push the whole string, `pop` a char, re-measure,
    /// repeat) is quadratic, and this text is attacker-controlled: notebook
    /// titles arrive through unauthenticated share uploads, so a 200 KB title
    /// pinned a worker for minutes and the disk cache could not absorb it,
    /// because the cache key is a hash of the very SVG this produces.
    #[must_use]
    pub fn ellipsize(&self, text: &str, size: f64, max_width: f64) -> String {
        let Some(scale) = self.scale(size) else {
            return ELLIPSIS.to_string();
        };

        let budget = max_width - self.width(ELLIPSIS, size);
        if budget <= 0.0 {
            return ELLIPSIS.to_string();
        }

        let mut used = 0.0;
        let mut end = 0;
        for (offset, c) in text.char_indices() {
            let advance = self.advance_units(c) * scale;
            if used + advance > budget {
                break;
            }
            used += advance;
            end = offset + c.len_utf8();
        }

        // Trailing space before an ellipsis reads as a typo. Strip a trailing
        // ellipsis too: `wrap` re-clips a line its own wrapping already
        // clipped, which otherwise renders a visible "……".
        let kept = text[..end].trim_end();
        let kept = kept.strip_suffix(ELLIPSIS).unwrap_or(kept).trim_end();

        let mut out = String::with_capacity(kept.len() + ELLIPSIS.len());
        out.push_str(kept);
        out.push_str(ELLIPSIS);
        out
    }

    /// Clips `text` to `max_width`, adding an ellipsis only when it did not
    /// already fit.
    #[must_use]
    pub fn clip(&self, text: &str, size: f64, max_width: f64) -> String {
        if self.width(text, size) <= max_width {
            return text.to_string();
        }
        self.ellipsize(text, size, max_width)
    }
}

// ── Lazily parsed singletons ────────────────────────────────────────────────

/// The regular sans face (Inter).
pub fn sans() -> &'static Font {
    static F: OnceLock<Font> = OnceLock::new();
    F.get_or_init(|| Font::parse(SANS_DATA))
}

/// The bold sans face (Inter Bold), used for the title and the wordmark.
pub fn sans_bold() -> &'static Font {
    static F: OnceLock<Font> = OnceLock::new();
    F.get_or_init(|| Font::parse(SANS_BOLD_DATA))
}

/// The monospace face (`JetBrains` Mono), used for the code excerpt.
pub fn mono() -> &'static Font {
    static F: OnceLock<Font> = OnceLock::new();
    F.get_or_init(|| Font::parse(MONO_DATA))
}

/// The font database handed to `usvg`, holding exactly the vendored faces.
///
/// Parsing three TTFs costs a few milliseconds, so it happens once per process
/// and every render clones the `Arc`.
pub fn database() -> Arc<fontdb::Database> {
    static DB: OnceLock<Arc<fontdb::Database>> = OnceLock::new();
    DB.get_or_init(|| {
        let mut db = fontdb::Database::new();
        db.load_font_data(SANS_DATA.to_vec());
        db.load_font_data(SANS_BOLD_DATA.to_vec());
        db.load_font_data(MONO_DATA.to_vec());
        db.set_sans_serif_family(SANS_FAMILY);
        db.set_monospace_family(MONO_FAMILY);
        Arc::new(db)
    })
    .clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vendored_faces_parse_and_report_the_expected_families() {
        // The SVG asks for these families by name; a font swap that renamed
        // them would silently fall back and change every card's layout.
        let db = database();
        let families: Vec<String> = db
            .faces()
            .flat_map(|f| f.families.iter().map(|(name, _)| name.clone()))
            .collect();

        assert!(
            families.iter().any(|f| f == SANS_FAMILY),
            "missing {SANS_FAMILY} in {families:?}"
        );
        assert!(
            families.iter().any(|f| f == MONO_FAMILY),
            "missing {MONO_FAMILY} in {families:?}"
        );
    }

    #[test]
    fn width_grows_with_text_and_scales_with_size() {
        let f = sans();
        assert!(f.width("mm", 40.0) > f.width("m", 40.0));
        assert!(f.width("", 40.0) == 0.0);

        // Linear in size: the advance sum is scaled, not re-measured.
        let single = f.width("ironpad", 20.0);
        let double = f.width("ironpad", 40.0);
        assert!((double - single * 2.0).abs() < 1e-9);
    }

    #[test]
    fn monospace_advances_are_uniform() {
        let m = mono();
        let a = m.width("i", 24.0);
        let b = m.width("W", 24.0);
        assert!(
            (a - b).abs() < 1e-9,
            "JetBrains Mono should advance uniformly: {a} vs {b}"
        );
    }

    #[test]
    fn wrap_breaks_on_words_and_respects_the_column() {
        let f = sans();
        let text = "Enzyme differentiates a timestepped ballistics simulation";
        let lines = f.wrap(text, 28.0, 300.0, 5);

        assert!(lines.len() > 1, "should have wrapped: {lines:?}");
        for line in &lines {
            assert!(
                f.width(line, 28.0) <= 300.0,
                "line over column: {line:?} = {}",
                f.width(line, 28.0)
            );
        }
        // Fits within the line budget, so nothing is marked as clipped.
        assert!(!lines.join(" ").contains(ELLIPSIS));
        assert_eq!(lines.join(" "), text);
    }

    #[test]
    fn wrap_marks_overflow_with_an_ellipsis_and_stays_in_the_column() {
        let f = sans();
        let text = "one two three four five six seven eight nine ten eleven twelve";
        let lines = f.wrap(text, 28.0, 200.0, 2);

        assert_eq!(lines.len(), 2);
        assert!(
            lines[1].ends_with(ELLIPSIS),
            "clipped text must say so: {lines:?}"
        );
        for line in &lines {
            assert!(f.width(line, 28.0) <= 200.0, "line over column: {line:?}");
        }
    }

    #[test]
    fn a_single_token_wider_than_the_column_is_clipped_not_overflowed() {
        let f = mono();
        let word = "a".repeat(500);
        let lines = f.wrap(&word, 21.0, 240.0, 3);

        assert_eq!(lines.len(), 1, "one token cannot become many lines");
        assert!(lines[0].ends_with(ELLIPSIS));
        assert!(f.width(&lines[0], 21.0) <= 240.0);
    }

    #[test]
    fn wrap_handles_degenerate_budgets() {
        let f = sans();
        assert!(f.wrap("anything", 28.0, 400.0, 0).is_empty());
        assert!(f.wrap("anything", 28.0, 0.0, 3).is_empty());
        assert!(f.wrap("   ", 28.0, 400.0, 3).is_empty());
    }

    #[test]
    fn ellipsize_never_splits_a_multibyte_char() {
        let f = sans();
        // Every char here is multi-byte; popping bytes would panic or produce
        // invalid UTF-8, and the result must still be a valid string.
        let clipped = f.ellipsize("→→→→→→→→→→→→→→→→→→→→", 40.0, 60.0);
        assert!(clipped.ends_with(ELLIPSIS));
        assert!(f.width(&clipped, 40.0) <= 60.0);
    }

    #[test]
    fn ellipsize_is_linear_not_quadratic() {
        // Regression guard for a remote DoS: `ellipsize` used to re-measure the
        // whole string after popping each char, so an attacker-supplied 200 KB
        // notebook title pinned a worker for minutes. 200k chars finished in
        // ~150 s before this fix; the budget below is ~1000x slack on the
        // linear version while still failing the quadratic one decisively.
        let f = sans_bold();
        let hostile = "W".repeat(200_000);

        let start = std::time::Instant::now();
        let out = f.ellipsize(&hostile, 60.0, 1056.0);
        let elapsed = start.elapsed();

        assert!(out.ends_with(ELLIPSIS));
        assert!(f.width(&out, 60.0) <= 1056.0);
        assert!(
            elapsed < std::time::Duration::from_secs(1),
            "ellipsize took {elapsed:?} on 200k chars, which means it went quadratic again"
        );
    }

    #[test]
    fn wrap_never_emits_a_double_ellipsis() {
        // `wrap` re-clips the last line that `wrap_all` may already have
        // clipped. Whenever the char dropped is wider than U+2026 the budget
        // reopens and a second one lands, rendering a visible "……".
        let f = sans_bold();
        let text = format!("hello {} tail", "W".repeat(40));
        let lines = f.wrap(&text, 60.0, 1056.0, 2);

        for line in &lines {
            assert!(
                !line.contains("\u{2026}\u{2026}"),
                "doubled ellipsis in {line:?}"
            );
        }
    }

    #[test]
    fn clip_leaves_fitting_text_untouched() {
        let f = sans();
        assert_eq!(f.clip("short", 24.0, 1000.0), "short");
        assert!(f.clip("short", 24.0, 10.0).ends_with(ELLIPSIS));
    }
}
