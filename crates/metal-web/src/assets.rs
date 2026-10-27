//! The theme and its fonts, baked into whichever binary is serving.
//!
//! Both MetalVJ surfaces — the studio and the app — are one page each,
//! and both are `include_str!`'d into their binary so there is no working
//! directory to get wrong and no half-installed asset folder to debug at a
//! venue. The theme is shared between them, so it is here rather than in
//! either.
//!
//! The fonts are bytes, not text, and are the only thing in MetalVJ the
//! browser is allowed to cache: they are fixed by the binary that served
//! them and cannot change under a running server.

/// The design tokens and component styles. Callers inline this into a
/// `<style>` block rather than linking it, which saves a round trip and — more
/// to the point — means the page can never render unstyled because a second
/// request lost a race with the first paint.
pub const THEME: &str = include_str!("../../../web/theme.css");

const REGULAR: &[u8] = include_bytes!("../../../web/fonts/plex-mono-regular.woff2");
const MEDIUM: &[u8] = include_bytes!("../../../web/fonts/plex-mono-medium.woff2");
const BOLD: &[u8] = include_bytes!("../../../web/fonts/plex-mono-bold.woff2");

/// The `<head>` contents every MetalVJ page shares: the charset, the
/// viewport, and the theme. Everything else a page needs is its own.
pub fn head(title: &str) -> String {
    format!(
        "<meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
         <title>{title}</title>\
         <style>{THEME}</style>"
    )
}

/// The two inert CRT layers, as markup. Every page wants them and none of
/// them should have to remember the class names.
pub const CRT: &str = "<div class=\"crt-scanlines\"></div><div class=\"crt-vignette\"></div>";

/// Resolves a static asset path. `None` means "not one of ours" — the caller
/// falls through to its own routes and then to 404.
pub fn get(path: &str) -> Option<(&'static str, &'static [u8])> {
    match path {
        "/theme.css" => Some(("text/css; charset=utf-8", THEME.as_bytes())),
        "/fonts/plex-mono-regular.woff2" => Some(("font/woff2", REGULAR)),
        "/fonts/plex-mono-medium.woff2" => Some(("font/woff2", MEDIUM)),
        "/fonts/plex-mono-bold.woff2" => Some(("font/woff2", BOLD)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_three_font_weights_the_theme_declares_are_all_servable() {
        // The @font-face rules and the routes are written in two places and
        // a mismatch is invisible: the browser silently falls back to a
        // system mono and the page looks nearly right.
        for weight in ["regular", "medium", "bold"] {
            let path = format!("/fonts/plex-mono-{weight}.woff2");
            // The theme names it *relative* — see the note there — and the
            // route serves it absolute. Both have to exist and they are not
            // spelled the same, which is the whole reason to check.
            let declared = format!("url(\"fonts/plex-mono-{weight}.woff2\")");
            assert!(THEME.contains(&declared), "theme declares no @font-face for {weight}");
            assert!(
                !THEME.contains(&format!("url(\"{path}\")")),
                "the {weight} font is declared root-absolute; it breaks behind a path prefix"
            );
            let (kind, bytes) = get(&path).unwrap_or_else(|| panic!("no route for {path}"));
            assert_eq!(kind, "font/woff2");
            assert!(bytes.len() > 1000, "{weight} is too small to be a real woff2");
            assert_eq!(&bytes[..4], b"wOF2", "{weight} is not woff2");
        }
    }

    #[test]
    fn the_theme_carries_the_metalvj_keys_and_not_the_inherited_ones() {
        assert!(THEME.contains("--accent:       #ff2e2e"), "the accent must be the red");
        assert!(!THEME.contains("#2fdcc0"), "the inherited blue-green must be gone");
    }

    #[test]
    fn an_unknown_path_falls_through() {
        assert!(get("/state").is_none());
        assert!(get("/fonts/../../etc/passwd").is_none());
    }
}
