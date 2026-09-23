//! URL path segments that are about to become filesystem path components.

/// Whether `s` is safe to join onto a directory as ONE path component: not
/// empty, no separator (`/` or `\`), and no `..` anywhere.
///
/// The one definition shared by the loaders that join a request segment onto
/// `data_dir`/`site_root` (`ironpad_app::server_fns`) and the server's
/// pre-filters in front of them (the OG card and oEmbed handlers). Those copies
/// had already drifted on the empty string, and a tightening applied to one
/// (rejecting NUL, say) would otherwise leave the rest behind.
#[must_use]
pub fn is_safe_path_segment(s: &str) -> bool {
    !s.is_empty() && !s.contains(['/', '\\']) && !s.contains("..")
}

#[cfg(test)]
mod tests {
    use super::is_safe_path_segment;

    #[test]
    fn accepts_a_plain_name_and_rejects_every_escape_shape() {
        for (segment, safe) in [
            ("welcome", true),
            ("a1b2c3d4e5f60718", true),
            ("cannon.ironpad", true),
            ("", false),
            ("a/b", false),
            ("a\\b", false),
            ("..", false),
            ("a..b", false),
            ("../etc/passwd", false),
        ] {
            assert_eq!(is_safe_path_segment(segment), safe, "{segment:?}");
        }
    }
}
