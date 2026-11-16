//! `data/router/gate.json` — the operator's song gate (`tools/song-gate`):
//! per song, a genre, which models are allowed, and which one is preferred.
//!
//! **Equivalence.** A model added after the gate was filled in has no tick in
//! any song. Rather than hand-editing every row, [`EQUIVALENT`] says which new
//! models stand in for an existing one: wherever the gate allows the original,
//! it allows its equivalents too — unless the row names the equivalent
//! explicitly, in which case the row wins.

use std::collections::BTreeMap;
use std::path::Path;

use serde::Deserialize;

/// The gate's file name under [`crate::DIR`].
pub const FILE: &str = "gate.json";

/// `(original, equivalents)`: a song that allows `original` also allows each
/// equivalent it does not mention itself.
pub const EQUIVALENT: &[(&str, &[&str])] = &[(
    "terrarium",
    &["parallax-cave", "parallax-sky", "parallax-fields", "parallax-city"],
)];

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Song {
    #[serde(default)]
    pub genre: String,
    #[serde(default)]
    pub allow: BTreeMap<String, bool>,
    #[serde(default)]
    pub prefer: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Gate {
    #[serde(default)]
    pub songs: BTreeMap<String, Song>,
}

impl Gate {
    pub fn parse(text: &str) -> serde_json::Result<Gate> {
        serde_json::from_str(text)
    }

    /// Reads `<data>/router/gate.json`; `None` when it is missing or unreadable.
    pub fn load(data: &Path) -> Option<Gate> {
        let text = metal_vfs::read_to_string(data.join(crate::DIR).join(FILE)).ok()?;
        Gate::parse(&text).ok()
    }

    /// Whether `song` allows model `id`, equivalence included.
    pub fn allows(&self, song: &str, id: &str) -> bool {
        self.songs.get(song).is_some_and(|s| allows(&s.allow, id))
    }

    /// Every model `song` allows, equivalents included, sorted.
    pub fn allowed(&self, song: &str) -> Vec<String> {
        let Some(s) = self.songs.get(song) else { return Vec::new() };
        let mut out: Vec<String> =
            s.allow.iter().filter(|(_, v)| **v).map(|(k, _)| k.clone()).collect();
        for (orig, eqs) in EQUIVALENT {
            if s.allow.get(*orig) == Some(&true) {
                for e in *eqs {
                    if !s.allow.contains_key(*e) {
                        out.push((*e).to_string());
                    }
                }
            }
        }
        out.sort();
        out.dedup();
        out
    }
}

/// The equivalence rule over one row's `allow` map.
pub fn allows(allow: &BTreeMap<String, bool>, id: &str) -> bool {
    if let Some(v) = allow.get(id) {
        return *v;
    }
    EQUIVALENT
        .iter()
        .any(|(orig, eqs)| eqs.contains(&id) && allow.get(*orig) == Some(&true))
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEXT: &str = r#"{ "version": 2, "models": [], "songs": {
        "a": { "genre": "pop", "allow": { "terrarium": true, "river": true } },
        "b": { "genre": "rock", "allow": { "terrarium": false, "river": true } },
        "c": { "genre": "edm", "allow": { "terrarium": true, "parallax-sky": false } }
    } }"#;

    #[test]
    fn the_parallax_worlds_are_allowed_wherever_terrario_is() {
        let g = Gate::parse(TEXT).unwrap();
        for e in EQUIVALENT.iter().find(|(o, _)| *o == "terrarium").unwrap().1 {
            assert!(g.allows("a", e), "{e} should follow terrario in a");
            assert!(!g.allows("b", e), "{e} should follow terrario's false in b");
        }
        assert!(g.allowed("a").contains(&"parallax-city".to_string()));
        assert!(!g.allowed("b").iter().any(|m| m.starts_with("parallax-")));
    }

    #[test]
    fn an_explicit_entry_wins_over_the_equivalence() {
        let g = Gate::parse(TEXT).unwrap();
        assert!(!g.allows("c", "parallax-sky"));
        assert!(g.allows("c", "parallax-cave"));
        assert!(!g.allowed("c").contains(&"parallax-sky".to_string()));
    }

    #[test]
    fn unknown_songs_and_models_allow_nothing() {
        let g = Gate::parse(TEXT).unwrap();
        assert!(!g.allows("nope", "terrarium"));
        assert!(!g.allows("a", "beam"));
        assert!(g.allowed("nope").is_empty());
    }

    #[test]
    fn the_real_gate_reads_and_every_terrario_song_gets_the_four_worlds() {
        let data = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data");
        let Some(g) = Gate::load(&data) else { return };
        let mut seen = 0;
        for (stem, s) in &g.songs {
            if s.allow.get("terrarium") == Some(&true) {
                seen += 1;
                for e in EQUIVALENT[0].1 {
                    assert!(g.allows(stem, e), "{stem} allows terrario but not {e}");
                }
            }
        }
        assert!(seen > 0, "no song in the real gate allows terrario");
    }
}
