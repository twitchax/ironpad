//! The one markup escaper for everything this crate writes into XML or HTML:
//! the OG card SVG, the sitemap, and oEmbed's iframe snippet.

/// Escapes text for XML/HTML element content or a double-quoted attribute,
/// dropping characters XML cannot represent at all.
///
/// Notebook titles are attacker-controlled on `/shared` and `/mutable`, so an
/// unescaped `<` would let a share inject arbitrary SVG (including a
/// `<script>`) into an image the server signs with its own hostname, or break
/// out of the iframe tag oEmbed hands a consumer to paste into its own page.
///
/// Escaping alone is not enough. XML 1.0 forbids the C0 controls outright, and
/// no entity can encode them, so a title carrying one made `usvg` reject the
/// whole document: `/og/{class}/{id}.png` answered 500 for that notebook
/// permanently, since the failure is deterministic in its content. The same
/// character in a sitemap `<loc>` makes crawlers discard the whole file. They
/// are dropped rather than escaped for that reason. Tab, newline, and carriage
/// return are the three XML permits and are kept.
///
/// `'` becomes `&#39;` rather than `&apos;`: the numeric form is valid in XML
/// 1.0 and in every HTML version, while `&apos;` is not an HTML4 entity, and
/// oEmbed's HTML lands in arbitrary consumer pages.
pub(crate) fn markup_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
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
    use super::markup_escape;

    #[test]
    fn escapes_every_markup_metacharacter() {
        assert_eq!(
            markup_escape(r#"<a href="x">&'"#),
            "&lt;a href=&quot;x&quot;&gt;&amp;&#39;"
        );
    }

    #[test]
    fn drops_xml_illegal_controls_and_keeps_the_legal_three() {
        assert_eq!(markup_escape("a\u{1}b\u{0}c\u{1f}d"), "abcd");
        assert_eq!(markup_escape("a\tb\nc\rd"), "a\tb\nc\rd");
    }

    #[test]
    fn plain_text_passes_through() {
        assert_eq!(
            markup_escape("The compiler fires a cannon"),
            "The compiler fires a cannon"
        );
    }
}
