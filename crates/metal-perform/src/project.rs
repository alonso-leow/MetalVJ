//! The `<song>.json` project: the one file a show is saved as.
//!
//! A [`Project`] is an ordered list of [`Page`]s, each a list of [`Clip`]s.
//! A clip is the unit a page holds, and it comes in two kinds:
//!
//! - [`Clip::Movement`] — a movement (a library motion) played on the page.
//!   A movement is a subset of a clip: it is the motion payload without any
//!   scheduling around it.
//! - [`Clip::Collection`] — a node collection shown from `start` to `end`
//!   seconds on the project's clock. This is what the old `Sequence` was:
//!   a timeline of collections. A sequence no longer exists on its own; it
//!   survives as the collection clips inside a project (see
//!   [`Project::from_sequence`]).
//!
//! Parsing is tolerant on purpose. Unknown keys are kept in `extra` and written
//! back on save, so a project written by a newer build is not stripped by an
//! older one, and missing keys take their defaults.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::sequence::Sequence;

/// A saved show: pages, a favourite flag, and whatever else the file carried.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Project {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub favourite: bool,
    #[serde(default)]
    pub pages: Vec<Page>,
    /// Keys this build does not know, kept verbatim so a save does not drop them.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// One page of a project: a name and the clips it plays, in order.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Page {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub clips: Vec<Clip>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// A clip on a page. Tagged by `kind` in the file.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Clip {
    /// A library movement, by its name.
    Movement { movement: String },
    /// A node collection, shown from `start` to `end` (exclusive), in seconds.
    Collection {
        collection: String,
        #[serde(default)]
        start: f32,
        #[serde(default)]
        end: f32,
    },
}

impl Project {
    /// Reads a project from its JSON text. Errors name what was wrong, so a
    /// hand-edited file says where, not just that it failed.
    pub fn parse(text: &str) -> Result<Project, String> {
        serde_json::from_str(text).map_err(|e| e.to_string())
    }

    /// Writes the project as pretty JSON.
    pub fn to_json(&self) -> Result<String, String> {
        serde_json::to_string_pretty(self).map_err(|e| e.to_string())
    }

    /// The one-page project a saved [`Sequence`] becomes: its entries, in
    /// order, as collection clips on a page named after the sequence.
    pub fn from_sequence(seq: &Sequence) -> Project {
        let clips = seq
            .entries
            .iter()
            .map(|e| Clip::Collection {
                collection: e.collection.clone(),
                start: e.start,
                end: e.end,
            })
            .collect();
        Project {
            name: seq.name.clone(),
            pages: vec![Page { name: seq.name.clone(), clips, extra: Map::new() }],
            ..Project::default()
        }
    }

    /// Names of the collections this project's collection clips refer to,
    /// in first-seen order and without repeats.
    pub fn collections(&self) -> Vec<&str> {
        let mut out: Vec<&str> = Vec::new();
        for clip in self.pages.iter().flat_map(|p| p.clips.iter()) {
            if let Clip::Collection { collection, .. } = clip {
                if !out.contains(&collection.as_str()) {
                    out.push(collection);
                }
            }
        }
        out
    }
}

/// A clip preset holding one movement, for a page to start from. Movements
/// that were saved on their own become these; nothing is lost in the move.
pub fn movement_preset(movement: &str) -> Clip {
    Clip::Movement { movement: movement.to_string() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sequence::SequenceEntry;

    fn seq() -> Sequence {
        Sequence {
            id: "s1".into(),
            name: "Opening".into(),
            entries: vec![
                SequenceEntry { collection: "red".into(), start: 0.0, end: 4.0 },
                SequenceEntry { collection: "blue".into(), start: 4.0, end: 8.0 },
            ],
        }
    }

    #[test]
    fn an_empty_object_is_an_empty_project() {
        let p = Project::parse("{}").unwrap();
        assert!(p.pages.is_empty());
        assert!(!p.favourite);
        assert!(p.name.is_empty());
    }

    #[test]
    fn a_full_project_round_trips() {
        let text = r#"{
            "name": "Song",
            "favourite": true,
            "pages": [
                {"name": "Intro", "clips": [
                    {"kind": "movement", "movement": "pulse"},
                    {"kind": "collection", "collection": "red", "start": 0.0, "end": 2.5}
                ]}
            ]
        }"#;
        let p = Project::parse(text).unwrap();
        assert!(p.favourite);
        assert_eq!(p.pages[0].clips[0], Clip::Movement { movement: "pulse".into() });
        assert_eq!(
            p.pages[0].clips[1],
            Clip::Collection { collection: "red".into(), start: 0.0, end: 2.5 }
        );
        let again = Project::parse(&p.to_json().unwrap()).unwrap();
        assert_eq!(again, p);
    }

    #[test]
    fn unknown_keys_survive_a_save() {
        let text = r#"{"name":"A","bpm":120,"pages":[{"name":"p","clips":[],"x":1}]}"#;
        let p = Project::parse(text).unwrap();
        let out: Value = serde_json::from_str(&p.to_json().unwrap()).unwrap();
        assert_eq!(out["bpm"], 120);
        assert_eq!(out["pages"][0]["x"], 1);
    }

    #[test]
    fn a_collection_clip_without_times_takes_zero_span() {
        let p = Project::parse(
            r#"{"pages":[{"clips":[{"kind":"collection","collection":"c"}]}]}"#,
        )
        .unwrap();
        assert_eq!(
            p.pages[0].clips[0],
            Clip::Collection { collection: "c".into(), start: 0.0, end: 0.0 }
        );
    }

    #[test]
    fn an_unknown_clip_kind_is_named_in_the_error() {
        let err = Project::parse(r#"{"pages":[{"clips":[{"kind":"video"}]}]}"#).unwrap_err();
        assert!(err.contains("video"), "{err}");
    }

    #[test]
    fn a_sequence_becomes_one_page_of_collection_clips() {
        let p = Project::from_sequence(&seq());
        assert_eq!(p.name, "Opening");
        assert_eq!(p.pages.len(), 1);
        assert_eq!(p.pages[0].clips.len(), 2);
        assert_eq!(p.collections(), vec!["red", "blue"]);
    }

    #[test]
    fn collections_are_listed_once_each() {
        let mut p = Project::from_sequence(&seq());
        let again = p.pages[0].clips[0].clone();
        p.pages[0].clips.push(again);
        assert_eq!(p.collections(), vec!["red", "blue"]);
    }

    #[test]
    fn a_movement_preset_is_a_movement_clip() {
        assert_eq!(movement_preset("pulse"), Clip::Movement { movement: "pulse".into() });
    }
}
