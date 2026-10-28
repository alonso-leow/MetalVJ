//! The panels: where each region of the shell lives and what it is called.
//!
//! `S.U8` laid the four out (header · grid, the focused-one-grows property);
//! `S.U9`–`S.U12` fill each in, one lote per region, and every one of them
//! reaches the app through `crate::call` — a panel that calls a handler body
//! or does I/O of its own is the second path regla 6 forbids
//! (`plan-sonnet.md` § S.U, *regla transversal*). `S.U9` (`nodes.rs`) is the
//! first to actually do that; this file itself still imports nothing from
//! `call` — it only declares the submodule and the state every later panel
//! shares.

pub mod builtins;
pub mod chat;
pub mod dials;
pub mod lanes;
pub mod library;
pub mod nodes;
pub mod pads;
pub mod pairing;
pub mod properties;
pub mod props_colour;
pub mod props_shader;
pub mod props_venue;
pub mod scenes;
pub mod sequence;
pub mod settings;
pub mod venue_canvas;
pub mod venue_editor;
pub mod venue_map;
pub mod venue_popup;
pub mod viewport;

/// The four regions of the shell. `#panel-preview` is not one of them —
/// it is externalised to `fw-monitor` (`plan-opus.md` § O.12.e, `S.U16`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Region {
    Nodes,
    Lanes,
    Library,
    Properties,
}

impl Region {
    /// The four, in the order the grid reads them: top row left to right,
    /// then the bottom row.
    pub const ALL: [Region; 4] = [
        Region::Properties,
        Region::Nodes,
        Region::Library,
        Region::Lanes,
    ];

    /// A stable id salt for the region's widgets. Never shown, never
    /// translated — the restyle gate compares trees by these.
    pub fn id(self) -> &'static str {
        match self {
            Region::Nodes => "shell/region/nodes",
            Region::Lanes => "shell/region/lanes",
            Region::Library => "shell/region/library",
            Region::Properties => "shell/region/properties",
        }
    }

    /// The kicker the region shows — the same words the old HTML dashboard's
    /// `.panel__head .kicker` used (page deleted 15-sept-2026, S.U18).
    pub fn title(self) -> &'static str {
        match self {
            Region::Nodes => "graph",
            Region::Lanes => "lanes / overlays",
            Region::Library => "library",
            Region::Properties => "properties",
        }
    }
}

/// What the graph canvas (`S.U9`) has selected — a node, an edge, or
/// nothing. Lives on [`super::Shell`] (`shell.selection`), not inside
/// [`nodes::PanelState`], precisely so a panel that is not `nodes` can read
/// it without reaching into another panel's private state — `S.U12`'s
/// properties panel is the reason this field exists at all
/// (`plan-sonnet.md` § S.U9's scope, point 1: "the selection lives in a
/// shared field on `Shell`").
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Selection {
    None,
    Node(String),
    Edge(String),
    /// A file picked in the library's `archivos` (2026-09-30): the properties
    /// panel answers the library as well as the canvas.
    File(FileRef),
}

/// A library file, as a selection: enough for the properties panel to show
/// it without asking the library panel anything.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileRef {
    pub name: String,
    pub path: String,
    pub kind: String,
}

/// An `archivos` card in flight (22-sept-2026, tarde): the payload a drag
/// from the library carries to the three drop targets — the nodes canvas,
/// the overlays grid and the lanes scratch. `kind` is the same word
/// `files_column` already reads (`audio|video|image|sequence|other`), `path`
/// is the absolute path the card resolved from the library's cwd, and `name`
/// is the bare file name a new node's label or a load gesture wants.
///
/// It is a plain value, not a reference into [`library::PanelState`]: the
/// drag outlives the frame that started it, so the payload has to own
/// everything a drop reads.
#[derive(Clone, Debug)]
pub struct FileDrag {
    pub name: String,
    pub path: String,
    pub kind: String,
    /// A video's length, when the library knows it — how long a clip it
    /// makes on the timeline.
    pub seconds: Option<f64>,
    /// A scene of the video rather than all of it (2026-10-01, the library's
    /// Scenes route): `(scene id, start, end)` in seconds. `path` is still
    /// the whole clip; `seconds` is the scene's length.
    pub scene: Option<(String, f64, f64)>,
}

impl FileDrag {
    /// The `metal-graph` source kind an image/video/audio drop becomes: a still
    /// for an image, a reel clip for a video, an audio-file node for audio.
    /// `None` for a sequence or anything else — those are not graph sources.
    pub fn source_kind(&self) -> Option<&'static str> {
        match self.kind.as_str() {
            "image" => Some("still"),
            "video" => Some("reel.clip"),
            "audio" => Some("audio.file"),
            "folder" => Some("reel.batch"),
            _ => None,
        }
    }

}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_region_is_listed_once_with_a_distinct_id_and_title() {
        let mut ids = std::collections::HashSet::new();
        let mut titles = std::collections::HashSet::new();
        for r in Region::ALL {
            assert!(ids.insert(r.id()), "duplicate id for {r:?}");
            assert!(titles.insert(r.title()), "duplicate title for {r:?}");
        }
        for r in [
            Region::Nodes,
            Region::Lanes,
            Region::Library,
            Region::Properties,
        ] {
            assert!(Region::ALL.contains(&r), "{r:?} missing from Region::ALL");
        }
    }
}
