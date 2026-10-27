//! The shell's UI language.
//!
//! **English is the source text.** Every string a panel paints is written in
//! English in the code and goes through [`tr`] (or [`trf`] when it carries
//! values, [`tr_dyn`] when it is not a literal, e.g. a catalog label the door
//! answered). Every other language is a folder under `crates/metal-dash/locales/`,
//! one TOML file per shell module, each a flat map from the English text to its
//! translation:
//!
//! ```toml
//! "save as sequence" = "<translation>"
//! "{n} files" = "<translation of {n} files>"
//! ```
//!
//! A string with no entry falls back to English, so a missing translation is
//! never a blank label. The files are embedded at build time, so the binary
//! needs nothing on disk. The tests below keep the tables honest: every file
//! parses, one English key never carries two translations, and every `tr`/`trf`
//! literal in the shell has a Spanish entry.
//!
//! The active language is process-wide. The shell sets it from
//! `data/settings.json`'s `language` (written through `POST /settings` by the
//! settings overlay); a fresh install is English.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::OnceLock;

/// A language the shell can paint in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Lang {
    En,
    Es,
}

impl Lang {
    /// Every language, in selector order.
    pub const ALL: [Lang; 2] = [Lang::En, Lang::Es];

    /// The code stored in `data/settings.json`.
    pub fn code(self) -> &'static str {
        match self {
            Lang::En => "en",
            Lang::Es => "es",
        }
    }

    pub fn parse(code: &str) -> Option<Lang> {
        match code {
            "en" => Some(Lang::En),
            "es" => Some(Lang::Es),
            _ => None,
        }
    }

    /// The language's name in itself: what a selector shows, never translated.
    pub fn native_name(self) -> &'static str {
        match self {
            Lang::En => "English",
            Lang::Es => "Español",
        }
    }

    fn index(self) -> u8 {
        match self {
            Lang::En => 0,
            Lang::Es => 1,
        }
    }
}

static CURRENT: AtomicU8 = AtomicU8::new(0);

/// Switch the whole shell to `lang` from the next painted frame on.
pub fn set_lang(lang: Lang) {
    CURRENT.store(lang.index(), Ordering::Relaxed);
}

/// The language the shell paints in now.
pub fn lang() -> Lang {
    match CURRENT.load(Ordering::Relaxed) {
        1 => Lang::Es,
        _ => Lang::En,
    }
}

/// `(file stem, contents)` of every Spanish table, one per shell module. A
/// module with strings of its own gets its own file: one line here, never a
/// section of somebody else's file.
///
/// `dynamic.toml` is the one table whose keys are not literals in the shell:
/// text the door answers (catalog labels, family names) and that a panel
/// looks up with [`tr_dyn`]. The orphan test skips it for that reason.
const ES_FILES: &[(&str, &str)] = &[
    ("shell", include_str!("../locales/es/shell.toml")),
    ("settings", include_str!("../locales/es/settings.toml")),
    ("pairing", include_str!("../locales/es/pairing.toml")),
    ("properties", include_str!("../locales/es/properties.toml")),
    ("describe", include_str!("../locales/es/describe.toml")),
    ("nodes", include_str!("../locales/es/nodes.toml")),
    ("lanes", include_str!("../locales/es/lanes.toml")),
    ("library", include_str!("../locales/es/library.toml")),
    ("chat", include_str!("../locales/es/chat.toml")),
    ("harness", include_str!("../locales/es/harness.toml")),
    ("loops", include_str!("../locales/es/loops.toml")),
    ("venue", include_str!("../locales/es/venue.toml")),
    ("shader", include_str!("../locales/es/shader.toml")),
    ("colour", include_str!("../locales/es/colour.toml")),
    ("script", include_str!("../locales/es/script.toml")),
    ("scenes", include_str!("../locales/es/scenes.toml")),
    ("sequence", include_str!("../locales/es/sequence.toml")),
    ("keybinds", include_str!("../locales/es/keybinds.toml")),
    ("listen", include_str!("../locales/es/listen.toml")),
    ("dynamic", include_str!("../locales/es/dynamic.toml")),
];

#[cfg(test)]
/// The stem of the table whose keys are not shell literals — see [`ES_FILES`].
const DYNAMIC_STEM: &str = "dynamic";

/// Merge `files` into one table. The same English key in two files must carry
/// the same translation; two different ones is an error, not last-one-wins.
fn load(files: &[(&str, &str)]) -> Result<HashMap<String, String>, String> {
    let mut out: HashMap<String, (String, String)> = HashMap::new();
    for (stem, text) in files {
        let table: toml::Table = text
            .parse()
            .map_err(|e| format!("locales/es/{stem}.toml: {e}"))?;
        for (key, value) in table {
            let Some(value) = value.as_str() else {
                return Err(format!("locales/es/{stem}.toml: {key:?} is not a string"));
            };
            if let Some((prev, prev_stem)) = out.get(&key) {
                if prev != value {
                    return Err(format!(
                        "{key:?} is {prev:?} in locales/es/{prev_stem}.toml and {value:?} in locales/es/{stem}.toml"
                    ));
                }
            }
            out.insert(key, (value.to_string(), (*stem).to_string()));
        }
    }
    Ok(out.into_iter().map(|(k, (v, _))| (k, v)).collect())
}

fn table(lang: Lang) -> Option<&'static HashMap<String, String>> {
    static ES: OnceLock<HashMap<String, String>> = OnceLock::new();
    match lang {
        Lang::En => None,
        Lang::Es => Some(ES.get_or_init(|| {
            load(ES_FILES).unwrap_or_else(|e| {
                eprintln!("locales: {e}; painting English");
                HashMap::new()
            })
        })),
    }
}

/// `en` in the active language, or `en` itself when the table has no entry.
pub fn tr(en: &'static str) -> &'static str {
    match table(lang()).and_then(|t| t.get(en)) {
        Some(s) => s.as_str(),
        None => en,
    }
}

/// [`tr`] for text that is not a literal in the shell: a catalog label or a
/// family name the door answered. Its Spanish lives in `dynamic.toml`.
pub fn tr_dyn(en: &str) -> String {
    match table(lang()).and_then(|t| t.get(en)) {
        Some(s) => s.clone(),
        None => en.to_string(),
    }
}

/// Marks an English literal that is painted later through [`tr`] from a
/// variable — a `const` table of labels, for instance — so the tests know the
/// literal is a translation key. Returns its argument unchanged.
pub const fn tr_key(en: &'static str) -> &'static str {
    en
}

/// [`tr`], then every `{name}` placeholder replaced by its value in `args`.
/// Placeholder names survive translation, so a language may move them:
/// `trf("{n} files", &[("n", &count)])`.
pub fn trf(en: &'static str, args: &[(&str, &dyn std::fmt::Display)]) -> String {
    let mut s = tr(en).to_string();
    for (name, value) in args {
        s = s.replace(&format!("{{{name}}}"), &value.to_string());
    }
    s
}

/// Restores English when a test that switched language ends, pass or fail,
/// so no other test paints in Spanish by accident.
#[cfg(test)]
pub(crate) struct LangGuard;

#[cfg(test)]
impl Drop for LangGuard {
    fn drop(&mut self) {
        set_lang(Lang::En);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use std::path::{Path, PathBuf};

    fn shell_sources() -> Vec<(PathBuf, String)> {
        fn walk(dir: &Path, out: &mut Vec<(PathBuf, String)>) {
            let mut entries: Vec<_> = metal_vfs::read_dir(dir)
                .unwrap_or_else(|e| panic!("read_dir {}: {e}", dir.display()))
                .map(|e| e.expect("dir entry").path())
                .collect();
            entries.sort();
            for path in entries {
                if metal_vfs::is_dir(&path) {
                    walk(&path, out);
                } else if path.extension().is_some_and(|x| x == "rs") {
                    let src = metal_vfs::read_to_string(&path)
                        .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
                    out.push((path, src));
                }
            }
        }
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src").join("shell");
        let mut out = Vec::new();
        walk(&root, &mut out);
        out
    }

    /// Reads the Rust string literal that starts right after an opening `"`,
    /// resolving the escapes a UI string can carry.
    fn literal_at(s: &str) -> Option<String> {
        let mut out = String::new();
        let mut chars = s.chars();
        while let Some(c) = chars.next() {
            match c {
                '"' => return Some(out),
                '\\' => match chars.next()? {
                    'n' => out.push('\n'),
                    't' => out.push('\t'),
                    '\\' => out.push('\\'),
                    '"' => out.push('"'),
                    '\'' => out.push('\''),
                    '0' => out.push('\0'),
                    '\n' => {
                        // A line continuation: skip the next line's indentation.
                        let rest: String = chars.clone().collect();
                        let skipped = rest.len() - rest.trim_start().len();
                        for _ in 0..skipped {
                            chars.next();
                        }
                    }
                    other => {
                        out.push('\\');
                        out.push(other);
                    }
                },
                c => out.push(c),
            }
        }
        None
    }

    /// Every literal passed to `tr("…")`, `trf("…"` or `tr_key("…")` in `src`.
    fn tr_literals(src: &str) -> Vec<String> {
        let mut out = Vec::new();
        for call in ["tr(", "trf(", "tr_key("] {
            let mut from = 0;
            while let Some(at) = src[from..].find(call) {
                let start = from + at;
                let before = src[..start].chars().next_back();
                let is_call = before.is_none_or(|c| !(c.is_alphanumeric() || c == '_'));
                let body = start + call.len();
                if is_call {
                    // The literal may follow the opening paren on the next
                    // line: rustfmt wraps long `tr("…")`/`trf("…")` calls, and
                    // the key is still the same.
                    let rest = src[body..].trim_start_matches([' ', '\n', '\r', '\t']);
                    if let Some(after_quote) = rest.strip_prefix('"') {
                        if let Some(lit) = literal_at(after_quote) {
                            out.push(lit);
                        }
                    }
                }
                from = body;
            }
        }
        out
    }

    #[test]
    fn every_spanish_table_parses_and_no_key_has_two_translations() {
        load(ES_FILES).unwrap();
    }

    #[test]
    fn two_files_disagreeing_about_one_key_is_an_error() {
        let err = load(&[("a", "\"x\" = \"one\""), ("b", "\"x\" = \"two\"")]).unwrap_err();
        assert!(err.contains("locales/es/a.toml") && err.contains("locales/es/b.toml"), "{err}");
        load(&[("a", "\"x\" = \"one\""), ("b", "\"x\" = \"one\"")]).unwrap();
    }

    #[test]
    fn a_missing_entry_falls_back_to_english() {
        let _guard = LangGuard;
        set_lang(Lang::Es);
        assert_eq!(tr("a string no table will ever carry"), "a string no table will ever carry");
    }

    #[test]
    fn trf_fills_placeholders_after_translating() {
        let _guard = LangGuard;
        set_lang(Lang::En);
        assert_eq!(trf("{n} of {m}", &[("n", &3), ("m", &"all")]), "3 of all");
    }

    #[test]
    fn language_codes_round_trip() {
        for l in Lang::ALL {
            assert_eq!(Lang::parse(l.code()), Some(l));
        }
        assert_eq!(Lang::parse("fr"), None);
    }

    #[test]
    fn the_literal_scanner_reads_calls_and_skips_lookalikes() {
        let src = r#"a(tr("save")); b(trf("{n} files", &[])); c.attr("no"); x = str("no"); const L: &str = tr_key("later");"#;
        assert_eq!(
            tr_literals(src),
            vec!["save".to_string(), "{n} files".to_string(), "later".to_string()]
        );
    }

    /// Every `tr`/`trf` literal in the shell has a Spanish entry: switching to
    /// Spanish never leaves an English label behind.
    #[test]
    fn every_tr_literal_in_the_shell_has_a_spanish_entry() {
        let es = load(ES_FILES).unwrap();
        let mut missing = BTreeSet::new();
        for (path, src) in shell_sources() {
            if path.ends_with("i18n.rs") {
                continue;
            }
            for lit in tr_literals(&src) {
                if !es.contains_key(&lit) {
                    let file = path.file_name().unwrap().to_string_lossy().into_owned();
                    missing.insert(format!("{file}: {lit:?}"));
                }
            }
        }
        assert!(
            missing.is_empty(),
            "add these to the module's locales/es/<module>.toml:\n{}",
            missing.into_iter().collect::<Vec<_>>().join("\n")
        );
    }

    /// Every entry outside `dynamic.toml` is still used by some `tr`/`trf`
    /// literal, so a renamed string cannot leave a dead translation behind.
    #[test]
    fn no_spanish_entry_is_orphaned() {
        let mut used = BTreeSet::new();
        for (path, src) in shell_sources() {
            if path.ends_with("i18n.rs") {
                continue;
            }
            used.extend(tr_literals(&src));
        }
        let mut orphans = Vec::new();
        for (stem, text) in ES_FILES {
            if *stem == DYNAMIC_STEM {
                continue;
            }
            let table: toml::Table = text.parse().unwrap();
            for key in table.keys() {
                if !used.contains(key) {
                    orphans.push(format!("locales/es/{stem}.toml: {key:?}"));
                }
            }
        }
        assert!(orphans.is_empty(), "no tr(\"…\") uses these any more:\n{}", orphans.join("\n"));
    }
}
