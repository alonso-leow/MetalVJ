//! Known controllers' faces: every key, pad, knob, fader and button of a box,
//! where it sits on the panel and what it sends out of the box — so the grid
//! can take the box's exact shape and come pre-bound to it.
//!
//! Positions are in sixths of a grid cell (the surface's sub-cells), `[x, y,
//! w, h]` from the top-left, so a half-width strip or a half-height button
//! sits where it does on the hardware.
//!
//! What a control does on the surface is its `role`, a surface role tag
//! (`rec`, `fn`, `speed-dial`, `page-next`, `fn-3`, …). A control without
//! one is a **slot**: the grid's own buttons fill the pads page by page, its
//! faders the knobs and faders. The keys are the piano.
//!
//! `input` is what the control sends with the box's factory settings (a
//! channel-qualified id, [`crate::Msg::input_on_channel`], when a pad shares
//! its notes with the keys; a bare one otherwise). `None` is a control that
//! acts inside the box and sends nothing — it still works from the screen,
//! and the cell's `learn` binds it if the box is set to send.

/// What a control is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    /// A keyboard: `count` keys from note `base`.
    Keys { base: u8, count: u8 },
    Pad,
    /// An absolute pot.
    Knob,
    /// An endless encoder (relative values).
    Encoder,
    Fader,
    /// A touch strip.
    Strip,
    Button,
    /// The maker's mark — painted, not a control.
    Logo,
}

/// One control on a box's panel.
#[derive(Clone, Debug, PartialEq)]
pub struct Control {
    /// Unique within the box: `pad-1`, `pot-3`, `play`.
    pub id: String,
    /// What the panel prints on or beside it.
    pub label: String,
    pub part: Part,
    /// `[x, y, w, h]` in sixths of a cell.
    pub rect: [u16; 4],
    /// What it sends by default (`note:36@10`, `cc:21`, `bend`).
    pub input: Option<String>,
    /// The surface role tag it plays; `None` is a slot (or the keys).
    pub role: Option<String>,
}

/// One box.
#[derive(Clone, Debug, PartialEq)]
pub struct Device {
    pub id: &'static str,
    pub name: &'static str,
    /// The part of an ALSA port name that is this box.
    pub port: &'static str,
    /// The grid it needs, in cells.
    pub cols: u8,
    pub rows: u8,
    /// What the operator should know: the box mode the inputs assume, the
    /// controls that send nothing.
    pub note: &'static str,
    pub controls: Vec<Control>,
}

impl Device {
    /// Whether `port` (an ALSA port name) is this box.
    pub fn is_port(&self, port: &str) -> bool {
        port.to_lowercase().contains(&self.port.to_lowercase())
    }

    pub fn count(&self, part: fn(&Part) -> bool) -> usize {
        self.controls.iter().filter(|c| part(&c.part)).count()
    }
}

/// Every box the grid can take the shape of.
pub fn catalogue() -> Vec<Device> {
    vec![flkey_mini(), nanokontrol2(), apc_mini_mk2(), launchpad_mini_mk3()]
}

/// The box `id` names.
pub fn device(id: &str) -> Option<Device> {
    catalogue().into_iter().find(|d| d.id == id)
}

/// The box on `port`, if it is a known one.
pub fn for_port(port: &str) -> Option<Device> {
    catalogue().into_iter().find(|d| d.is_port(port))
}

/// A control from cell units (`0.5` is a half): `[x, y, w, h]`.
fn at(x: f32, y: f32, w: f32, h: f32) -> [u16; 4] {
    let u = |v: f32| (v * 6.0).round() as u16;
    [u(x), u(y), u(w), u(h)]
}

fn control(id: impl Into<String>, label: impl Into<String>, part: Part, rect: [u16; 4]) -> Control {
    Control { id: id.into(), label: label.into(), part, rect, input: None, role: None }
}

impl Control {
    fn sends(mut self, input: impl Into<String>) -> Control {
        self.input = Some(input.into());
        self
    }

    fn plays(mut self, role: impl Into<String>) -> Control {
        self.role = Some(role.into());
        self
    }
}

/// Novation FLkey Mini (Novation's hardware overview, user guide v4 p. 8):
/// the pitch and modulation strips at the far left; Shift, Transpose and
/// Octave +/− in the column beside them; eight pots over two rows of eight
/// pads; Channel Rack ▲/▼ right of the pads, then Scale and Note Repeat
/// over Play and Record; 25 mini keys under all of it.
fn flkey_mini() -> Device {
    let mut c = vec![
        control("pitch", "pitch", Part::Strip, at(0.0, 0.0, 0.5, 3.0)).sends("bend"),
        control("mod", "mod", Part::Strip, at(0.5, 0.0, 0.5, 3.0)).sends("cc:1"),
        control("shift", "shift", Part::Button, at(1.0, 0.0, 1.0, 1.0)).plays("fn"),
        control("transpose", "transpose", Part::Button, at(1.0, 1.0, 1.0, 1.0)).plays("folder"),
        control("octave-up", "octave +", Part::Button, at(1.0, 2.0, 1.0, 0.5)).plays("octave-up"),
        control("octave-down", "octave −", Part::Button, at(1.0, 2.5, 1.0, 0.5)).plays("octave-down"),
    ];
    // The pots send CC 21–28 in the Custom pot mode, the one mode the box has
    // without FL Studio. The first three are the surface's own dials.
    for i in 0..8u8 {
        let pot = control(format!("pot-{}", i + 1), format!("pot {}", i + 1), Part::Knob, at(2.0 + i as f32, 0.0, 1.0, 1.0))
            .sends(format!("cc:{}", 21 + i));
        c.push(match i {
            0 => pot.plays("speed-dial"),
            1 => pot.plays("search-dial"),
            2 => pot.plays("fn-dial"),
            _ => pot,
        });
    }
    // The pads on channel 10, two 4×2 drum blocks: 40–43 48–51 over
    // 36–39 44–47 (the FPC halves the guide describes).
    const TOP: [u8; 8] = [40, 41, 42, 43, 48, 49, 50, 51];
    const BOTTOM: [u8; 8] = [36, 37, 38, 39, 44, 45, 46, 47];
    for (row, notes) in [TOP, BOTTOM].iter().enumerate() {
        for (i, n) in notes.iter().enumerate() {
            let k = row * 8 + i + 1;
            c.push(control(format!("pad-{k}"), "", Part::Pad, at(2.0 + i as f32, 1.0 + row as f32, 1.0, 1.0)).sends(format!("note:{n}@10")));
        }
    }
    c.extend([
        control("rack-up", "rack ▲", Part::Button, at(10.0, 1.0, 1.0, 1.0)).sends("cc:104").plays("page-prev"),
        control("rack-down", "rack ▼", Part::Button, at(10.0, 2.0, 1.0, 1.0)).sends("cc:105").plays("page-next"),
        control("scale", "scale", Part::Button, at(11.0, 1.0, 1.0, 1.0)).plays("load"),
        control("note-repeat", "repeat", Part::Button, at(12.0, 1.0, 1.0, 1.0)).plays("drop-rec"),
        control("play", "▶", Part::Button, at(11.0, 2.0, 1.0, 1.0)).sends("cc:115").plays("play"),
        control("record", "●", Part::Button, at(12.0, 2.0, 1.0, 1.0)).sends("cc:117").plays("rec"),
        control("logo", "FLkey mini", Part::Logo, at(10.0, 0.0, 3.0, 1.0)),
        control("keys", "keys", Part::Keys { base: 48, count: 25 }, at(0.0, 3.0, 13.0, 2.0)),
    ]);
    Device {
        id: "flkey-mini",
        name: "Novation FLkey Mini",
        port: "flkey mini",
        cols: 13,
        rows: 5,
        note: "Pads in Custom pad mode, pots in Custom pot mode (Shift + the Custom pads). \
               Shift, Transpose, Octave, Scale and Note Repeat act inside the keyboard and send no MIDI on their own: \
               they work from the screen, or learn them if Components sets them to send.",
        controls: c,
    }
}

/// Korg nanoKONTROL2 (factory CC mode, channel 1): the transport block at
/// the left — Track ◀ ▶; Cycle, Marker Set ◀ ▶; ◀◀ ▶▶ ■ ▶ ● — then eight
/// channel strips, each a knob over a fader with Solo/Mute/Rec beside it.
fn nanokontrol2() -> Device {
    let mut c = vec![
        control("track-prev", "track ◀", Part::Button, at(0.0, 0.0, 1.0, 1.0)).sends("cc:58").plays("page-prev"),
        control("track-next", "track ▶", Part::Button, at(1.0, 0.0, 1.0, 1.0)).sends("cc:59").plays("page-next"),
        control("cycle", "cycle", Part::Button, at(0.0, 1.0, 1.0, 1.0)).sends("cc:46").plays("fn"),
        control("marker-set", "set", Part::Button, at(2.0, 1.0, 1.0, 1.0)).sends("cc:60").plays("load"),
        control("marker-prev", "marker ◀", Part::Button, at(3.0, 1.0, 1.0, 1.0)).sends("cc:61").plays("folder"),
        control("marker-next", "marker ▶", Part::Button, at(4.0, 1.0, 1.0, 1.0)).sends("cc:62"),
        control("rew", "◀◀", Part::Button, at(0.0, 2.0, 1.0, 1.0)).sends("cc:43"),
        control("ff", "▶▶", Part::Button, at(1.0, 2.0, 1.0, 1.0)).sends("cc:44"),
        control("stop", "■", Part::Button, at(2.0, 2.0, 1.0, 1.0)).sends("cc:42").plays("drop-rec"),
        control("play", "▶", Part::Button, at(3.0, 2.0, 1.0, 1.0)).sends("cc:41").plays("play"),
        control("rec", "●", Part::Button, at(4.0, 2.0, 1.0, 1.0)).sends("cc:45").plays("rec"),
    ];
    let strip_x = |i: u8| 5.0 + 1.5 * i as f32;
    // S, M and R by rows, so the grid's buttons fill a row across before the next.
    for (row, (name, base)) in [("S", 32u8), ("M", 48), ("R", 64)].iter().enumerate() {
        for i in 0..8u8 {
            c.push(
                control(format!("{}-{}", name.to_lowercase(), i + 1), *name, Part::Pad, at(strip_x(i), row as f32, 0.5, 1.0))
                    .sends(format!("cc:{}", base + i)),
            );
        }
    }
    for i in 0..8u8 {
        c.push(control(format!("fader-{}", i + 1), format!("{}", i + 1), Part::Fader, at(strip_x(i) + 0.5, 1.0, 1.0, 2.0)).sends(format!("cc:{i}")));
    }
    for i in 0..8u8 {
        let knob = control(format!("knob-{}", i + 1), format!("knob {}", i + 1), Part::Knob, at(strip_x(i) + 0.5, 0.0, 1.0, 1.0))
            .sends(format!("cc:{}", 16 + i));
        c.push(match i {
            0 => knob.plays("speed-dial"),
            1 => knob.plays("search-dial"),
            2 => knob.plays("fn-dial"),
            _ => knob,
        });
    }
    Device {
        id: "nanokontrol2",
        name: "Korg nanoKONTROL2",
        port: "nanokontrol2",
        cols: 17,
        rows: 3,
        note: "Factory CC mode (not a DAW mode): hold SET MARKER + CYCLE while plugging in to return to it.",
        controls: c,
    }
}

/// Akai APC mini mk2: an 8×8 pad grid (notes 0–63 from the bottom-left),
/// the scene-launch column at its right (notes 112–119), the track buttons
/// under it (notes 100–107) with Shift (122) at the corner, and nine faders
/// (CC 48–56) along the foot.
fn apc_mini_mk2() -> Device {
    let mut c = Vec::new();
    for row in 0..8u8 {
        for col in 0..8u8 {
            let n = (7 - row) * 8 + col;
            c.push(control(format!("pad-{}", row * 8 + col + 1), "", Part::Pad, at(col as f32, row as f32, 1.0, 1.0)).sends(format!("note:{n}")));
        }
    }
    for row in 0..8u8 {
        c.push(
            control(format!("scene-{}", row + 1), "▶", Part::Button, at(8.0, row as f32, 1.0, 1.0))
                .sends(format!("note:{}", 112 + row))
                .plays(format!("fn-{}", row + 1)),
        );
    }
    let tracks = ["rec", "drop-rec", "play", "load", "folder", "page-prev", "page-next", ""];
    for (i, role) in tracks.iter().enumerate() {
        let b = control(format!("track-{}", i + 1), format!("{}", i + 1), Part::Button, at(i as f32, 8.0, 1.0, 1.0))
            .sends(format!("note:{}", 100 + i));
        c.push(if role.is_empty() { b } else { b.plays(*role) });
    }
    c.push(control("shift", "shift", Part::Button, at(8.0, 8.0, 1.0, 1.0)).sends("note:122").plays("fn"));
    for i in 0..9u8 {
        let f = control(format!("fader-{}", i + 1), if i == 8 { "master".into() } else { format!("{}", i + 1) }, Part::Fader, at(i as f32, 9.0, 1.0, 2.0))
            .sends(format!("cc:{}", 48 + i));
        c.push(if i == 8 { f.plays("speed-dial") } else { f });
    }
    Device {
        id: "apc-mini-mk2",
        name: "Akai APC mini mk2",
        port: "apc mini mk2",
        cols: 9,
        rows: 11,
        note: "Factory Note mode, channel 1.",
        controls: c,
    }
}

/// Novation Launchpad Mini MK3 in Programmer mode: the top row of arrows and
/// mode buttons (CC 91–98), the 8×8 grid (notes 11–88, row × 10 + column
/// from the bottom-left), the scene column at the right (CC 89 … 19).
fn launchpad_mini_mk3() -> Device {
    let mut c = Vec::new();
    let top = [
        ("▲", "page-prev"),
        ("▼", "page-next"),
        ("◀", "folder"),
        ("▶", "load"),
        ("session", "rec"),
        ("drums", "drop-rec"),
        ("keys", "play"),
        ("user", "fn"),
    ];
    for (i, (label, role)) in top.iter().enumerate() {
        c.push(control(format!("top-{}", i + 1), *label, Part::Button, at(i as f32, 0.0, 1.0, 1.0)).sends(format!("cc:{}", 91 + i)).plays(*role));
    }
    c.push(control("logo", "LP", Part::Logo, at(8.0, 0.0, 1.0, 1.0)));
    for row in 0..8u8 {
        for col in 0..8u8 {
            let n = (8 - row) * 10 + col + 1;
            c.push(control(format!("pad-{}", row * 8 + col + 1), "", Part::Pad, at(col as f32, 1.0 + row as f32, 1.0, 1.0)).sends(format!("note:{n}")));
        }
    }
    for row in 0..8u8 {
        let label = if row == 7 { "stop · solo · mute" } else { "▶" };
        c.push(
            control(format!("scene-{}", row + 1), label, Part::Button, at(8.0, 1.0 + row as f32, 1.0, 1.0))
                .sends(format!("cc:{}", (8 - row) * 10 + 9))
                .plays(format!("fn-{}", row + 1)),
        );
    }
    Device {
        id: "launchpad-mini-mk3",
        name: "Novation Launchpad Mini MK3",
        port: "lpminimk3",
        cols: 9,
        rows: 9,
        note: "Programmer mode: hold Session, press the orange pad, release Session.",
        controls: c,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn overlaps(a: [u16; 4], b: [u16; 4]) -> bool {
        a[0] < b[0] + b[2] && b[0] < a[0] + a[2] && a[1] < b[1] + b[3] && b[1] < a[1] + a[3]
    }

    #[test]
    fn every_box_fits_its_grid_without_overlaps_or_twin_ids() {
        for d in catalogue() {
            let (w, h) = (d.cols as u16 * 6, d.rows as u16 * 6);
            for (i, a) in d.controls.iter().enumerate() {
                assert!(a.rect[0] + a.rect[2] <= w && a.rect[1] + a.rect[3] <= h, "{} {} leaves the grid", d.id, a.id);
                assert!(a.rect[2] > 0 && a.rect[3] > 0, "{} {} has no size", d.id, a.id);
                for b in &d.controls[i + 1..] {
                    assert_ne!(a.id, b.id, "{}: twin id", d.id);
                    assert!(!overlaps(a.rect, b.rect), "{}: {} overlaps {}", d.id, a.id, b.id);
                }
            }
            let inputs: Vec<&String> = d.controls.iter().filter_map(|c| c.input.as_ref()).collect();
            for (i, a) in inputs.iter().enumerate() {
                assert!(!inputs[i + 1..].contains(a), "{}: {a} sent twice", d.id);
            }
        }
    }

    #[test]
    fn the_flkey_mini_is_the_whole_panel() {
        let d = device("flkey-mini").unwrap();
        assert_eq!(d.count(|p| matches!(p, Part::Pad)), 16);
        assert_eq!(d.count(|p| matches!(p, Part::Knob)), 8);
        assert_eq!(d.count(|p| matches!(p, Part::Strip)), 2);
        assert_eq!(d.count(|p| matches!(p, Part::Button)), 10);
        let keys = d.controls.iter().find(|c| matches!(c.part, Part::Keys { .. })).unwrap();
        assert_eq!(keys.part, Part::Keys { base: 48, count: 25 });
        assert!(for_port("FLkey Mini:FLkey Mini MIDI 1 24:0").is_some_and(|d| d.id == "flkey-mini"));
        assert!(for_port("Midi Through:Midi Through Port-0 14:0").is_none());
    }

    #[test]
    fn the_grid_boxes_number_their_pads_from_the_bottom_left() {
        let apc = device("apc-mini-mk2").unwrap();
        let first = |d: &Device, id: &str| d.controls.iter().find(|c| c.id == id).unwrap().input.clone().unwrap();
        assert_eq!(first(&apc, "pad-57"), "note:0", "bottom-left");
        assert_eq!(first(&apc, "pad-8"), "note:63", "top-right");
        let lp = device("launchpad-mini-mk3").unwrap();
        assert_eq!(first(&lp, "pad-57"), "note:11");
        assert_eq!(first(&lp, "pad-8"), "note:88");
        assert_eq!(first(&lp, "scene-1"), "cc:89");
    }
}
