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
    /// Who makes it (`Novation`, `Akai`, `Korg`), or `Generic` for a
    /// common layout no one box owns.
    pub maker: &'static str,
    /// The part of an ALSA port name that is this box (empty: a generic
    /// layout, never matched to a port).
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
        !self.port.is_empty() && port.to_lowercase().contains(&self.port.to_lowercase())
    }

    pub fn count(&self, part: fn(&Part) -> bool) -> usize {
        self.controls.iter().filter(|c| part(&c.part)).count()
    }
}

/// Every box the grid can take the shape of, maker by maker, the generic
/// layouts last.
pub fn catalogue() -> Vec<Device> {
    let mut all = vec![
        flkey_mini(),
        launchpad_mini_mk3(),
        launchpad_x(),
        launch_control_xl(),
        mpk_mini_mk3(),
        apc_mini_mk2(),
        midimix(),
        nanokontrol2(),
    ];
    all.extend(generic::all());
    all
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
        maker: "Novation",
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
        maker: "Korg",
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
        maker: "Akai",
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
    Device {
        id: "launchpad-mini-mk3",
        name: "Novation Launchpad Mini MK3",
        maker: "Novation",
        port: "lpminimk3",
        cols: 9,
        rows: 9,
        note: "Programmer mode: hold Session, press the orange pad, release Session.",
        controls: launchpad(["session", "drums", "keys", "user"], "stop · solo · mute"),
    }
}

/// Novation Launchpad X in Programmer mode: the Mini MK3's face and numbers,
/// its own mode buttons.
fn launchpad_x() -> Device {
    Device {
        id: "launchpad-x",
        name: "Novation Launchpad X",
        maker: "Novation",
        port: "launchpad x",
        cols: 9,
        rows: 9,
        note: "Programmer mode: hold Session, press the orange pad, release Session.",
        controls: launchpad(["session", "note", "custom", "capture"], "record arm"),
    }
}

/// A Launchpad's 9×9 in Programmer mode, `modes` the four mode buttons of
/// the top row, `last` the scene column's foot.
fn launchpad(modes: [&str; 4], last: &str) -> Vec<Control> {
    let mut c = Vec::new();
    let top = [
        ("▲", "page-prev"),
        ("▼", "page-next"),
        ("◀", "folder"),
        ("▶", "load"),
        (modes[0], "rec"),
        (modes[1], "drop-rec"),
        (modes[2], "play"),
        (modes[3], "fn"),
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
        let label = if row == 7 { last } else { "▶" };
        c.push(
            control(format!("scene-{}", row + 1), label, Part::Button, at(8.0, 1.0 + row as f32, 1.0, 1.0))
                .sends(format!("cc:{}", (8 - row) * 10 + 9))
                .plays(format!("fn-{}", row + 1)),
        );
    }
    c
}

/// Novation Launch Control XL (mk2), factory template 1: eight channels of
/// three knob rows (Send A CC 13–20, Send B 29–36, Pan 49–56) over a fader
/// (CC 77–84) and two button rows (Track Focus notes 41–44 57–60, Track
/// Control 73–76 89–92); at the right Send ▲▼ (CC 104/105), Track ◀▶
/// (CC 106/107), then Device, Mute, Solo and Record Arm (notes 105–108).
fn launch_control_xl() -> Device {
    let mut c = Vec::new();
    for (row, (name, base)) in [("send a", 13u8), ("send b", 29), ("pan", 49)].iter().enumerate() {
        for i in 0..8u8 {
            let knob = control(format!("{}-{}", name.replace(' ', "-"), i + 1), format!("{name} {}", i + 1), Part::Knob, at(i as f32, row as f32, 1.0, 1.0))
                .sends(format!("cc:{}", base + i));
            c.push(match (row, i) {
                (0, 0) => knob.plays("speed-dial"),
                (0, 1) => knob.plays("search-dial"),
                (0, 2) => knob.plays("fn-dial"),
                _ => knob,
            });
        }
    }
    for i in 0..8u8 {
        c.push(control(format!("fader-{}", i + 1), format!("{}", i + 1), Part::Fader, at(i as f32, 3.0, 1.0, 2.0)).sends(format!("cc:{}", 77 + i)));
    }
    const FOCUS: [u8; 8] = [41, 42, 43, 44, 57, 58, 59, 60];
    const CONTROL: [u8; 8] = [73, 74, 75, 76, 89, 90, 91, 92];
    for (row, (name, notes)) in [("focus", FOCUS), ("control", CONTROL)].iter().enumerate() {
        for (i, n) in notes.iter().enumerate() {
            c.push(control(format!("{name}-{}", i + 1), "", Part::Pad, at(i as f32, 5.0 + row as f32, 1.0, 1.0)).sends(format!("note:{n}")));
        }
    }
    c.extend([
        control("send-up", "send ▲", Part::Button, at(8.0, 0.0, 1.0, 1.0)).sends("cc:104").plays("page-prev"),
        control("send-down", "send ▼", Part::Button, at(8.0, 1.0, 1.0, 1.0)).sends("cc:105").plays("page-next"),
        control("track-prev", "◀", Part::Button, at(8.0, 2.0, 0.5, 1.0)).sends("cc:106").plays("folder"),
        control("track-next", "▶", Part::Button, at(8.5, 2.0, 0.5, 1.0)).sends("cc:107").plays("load"),
        control("device", "device", Part::Button, at(8.0, 3.0, 1.0, 1.0)).sends("note:105").plays("fn"),
        control("mute", "mute", Part::Button, at(8.0, 4.0, 1.0, 1.0)).sends("note:106").plays("drop-rec"),
        control("solo", "solo", Part::Button, at(8.0, 5.0, 1.0, 1.0)).sends("note:107").plays("play"),
        control("record-arm", "record arm", Part::Button, at(8.0, 6.0, 1.0, 1.0)).sends("note:108").plays("rec"),
    ]);
    Device {
        id: "launch-control-xl",
        name: "Novation Launch Control XL",
        maker: "Novation",
        port: "launch control xl",
        cols: 9,
        rows: 7,
        note: "Factory template 1 (the box's default).",
        controls: c,
    }
}

/// Akai MPK Mini MK3 (Akai's quickstart diagram): the thumbstick at the top
/// left over Arpeggiator / Tap Tempo, Octave − / +, Full Level / Note Repeat;
/// two rows of four pads (5–8 over 1–4); the display and Bank A/B, CC, Prog
/// Change, Prog Select over two rows of four knobs (K1–K8); 25 keys.
/// Program 1: pads notes 36–43 on channel 10, knobs CC 70–77, the stick's X
/// the pitch bend and its Y the mod wheel.
fn mpk_mini_mk3() -> Device {
    let mut c = vec![
        control("stick-x", "x", Part::Strip, at(0.0, 0.0, 1.0, 1.0)).sends("bend"),
        control("stick-y", "y", Part::Strip, at(1.0, 0.0, 1.0, 1.0)).sends("cc:1"),
    ];
    let left = [
        [("arp", "arp", "fn"), ("tap", "tap tempo", "rec")],
        [("octave-down", "oct −", "octave-down"), ("octave-up", "oct +", "octave-up")],
        [("full-level", "full level", "drop-rec"), ("note-repeat", "note repeat", "play")],
    ];
    // Three button rows in the two pad rows' height: two thirds of a cell each.
    for (row, pair) in left.iter().enumerate() {
        for (col, (id, label, role)) in pair.iter().enumerate() {
            let rect = [col as u16 * 6, 6 + row as u16 * 4, 6, 4];
            c.push(control(*id, *label, Part::Button, rect).plays(*role));
        }
    }
    for (row, first) in [(0usize, 5u8), (1, 1)] {
        for i in 0..4u8 {
            let pad = first + i;
            c.push(
                control(format!("pad-{pad}"), format!("pad {pad}"), Part::Pad, at(2.0 + i as f32, row as f32 * 1.5, 1.0, 1.5))
                    .sends(format!("note:{}@10", 35 + pad)),
            );
        }
    }
    c.push(control("display", "display", Part::Logo, at(6.0, 0.0, 1.0, 1.0)));
    for (i, (id, label, role)) in [
        ("bank", "bank a/b", "page-next"),
        ("cc", "cc", "folder"),
        ("prog-change", "prog change", "load"),
        ("prog-select", "prog select", "button"),
    ]
    .iter()
    .enumerate()
    {
        c.push(control(*id, *label, Part::Button, at(7.0 + i as f32, 0.0, 1.0, 1.0)).plays(*role));
    }
    for i in 0..8u8 {
        let (row, col) = (i / 4, i % 4);
        let knob = control(format!("k{}", i + 1), format!("k{}", i + 1), Part::Knob, at(6.5 + col as f32, 1.0 + row as f32, 1.0, 1.0))
            .sends(format!("cc:{}", 70 + i));
        c.push(match i {
            0 => knob.plays("speed-dial"),
            1 => knob.plays("search-dial"),
            2 => knob.plays("fn-dial"),
            _ => knob,
        });
    }
    c.push(control("keys", "keys", Part::Keys { base: 48, count: 25 }, at(0.0, 3.0, 11.0, 2.0)));
    Device {
        id: "mpk-mini-mk3",
        name: "Akai MPK Mini MK3",
        maker: "Akai",
        port: "mpk mini",
        cols: 11,
        rows: 5,
        note: "Program 1, pad bank A. Its buttons act inside the keyboard and send no MIDI: they work from the screen.",
        controls: c,
    }
}

/// Akai MIDImix: eight channels of three knobs, Mute over Rec Arm and a
/// fader; at the right Bank Left / Right, Solo and the master fader.
fn midimix() -> Device {
    let mut c = Vec::new();
    const KNOBS: [u8; 8] = [16, 20, 24, 28, 46, 50, 54, 58];
    const FADERS: [u8; 8] = [19, 23, 27, 31, 49, 53, 57, 61];
    for (i, base) in KNOBS.iter().enumerate() {
        for row in 0..3u8 {
            let knob = control(format!("knob-{}-{}", i + 1, row + 1), "", Part::Knob, at(i as f32, row as f32, 1.0, 1.0))
                .sends(format!("cc:{}", base + row));
            c.push(match (i, row) {
                (0, 0) => knob.plays("speed-dial"),
                (0, 1) => knob.plays("search-dial"),
                (0, 2) => knob.plays("fn-dial"),
                _ => knob,
            });
        }
    }
    for (row, (name, first)) in [("mute", 1u8), ("rec-arm", 3)].iter().enumerate() {
        for i in 0..8u8 {
            let label = if row == 0 { "mute" } else { "rec arm" };
            c.push(control(format!("{name}-{}", i + 1), label, Part::Pad, at(i as f32, 3.0 + row as f32 * 0.5, 1.0, 0.5)).sends(format!("note:{}", first + 3 * i)));
        }
    }
    for (i, cc) in FADERS.iter().enumerate() {
        c.push(control(format!("fader-{}", i + 1), format!("{}", i + 1), Part::Fader, at(i as f32, 4.0, 1.0, 2.0)).sends(format!("cc:{cc}")));
    }
    c.extend([
        control("logo", "MIDImix", Part::Logo, at(8.0, 0.0, 1.0, 2.0)),
        control("bank-left", "bank ◀", Part::Button, at(8.0, 2.0, 0.5, 1.0)).sends("note:25").plays("page-prev"),
        control("bank-right", "bank ▶", Part::Button, at(8.5, 2.0, 0.5, 1.0)).sends("note:26").plays("page-next"),
        control("solo", "solo", Part::Button, at(8.0, 3.0, 1.0, 1.0)).sends("note:27").plays("fn"),
        control("master", "master", Part::Fader, at(8.0, 4.0, 1.0, 2.0)).sends("cc:62"),
    ]);
    Device {
        id: "midimix",
        name: "Akai MIDImix",
        maker: "Akai",
        port: "midimix",
        cols: 9,
        rows: 6,
        note: "The box's default mapping (Mute mode).",
        controls: c,
    }
}

/// Common layouts no one box owns, named by what they have. Their numbers
/// are the usual defaults — knobs CC 21–28 (a second row 29–36), faders CC
/// 41–48 (a ninth 49), pads notes 36 up from the bottom-left, transport CC
/// 102/103 (page) and 115/117 (play/record) — set the box to send them, or
/// learn each control.
mod generic {
    use super::*;

    const NOTE: &str = "A common layout: set your controller to these numbers (knobs CC 21–28, faders CC 41–48, pads notes from 36, \
                        ◀ ▶ CC 102/103, play CC 115, record CC 117), or learn each control.";

    fn device(id: &'static str, name: &'static str, cols: u8, rows: u8, controls: Vec<Control>) -> Device {
        Device { id, name, maker: "Generic", port: "", cols, rows, note: NOTE, controls }
    }

    pub fn all() -> Vec<Device> {
        vec![
            device("generic-8-faders", "8 faders", 8, 3, faders(0.0, 0.0, 8, 3.0)),
            device("generic-8-knobs-8-faders", "8 knobs + 8 faders", 8, 4, [knobs(0.0, 0.0, 8, 1, true), faders(0.0, 1.0, 8, 3.0)].concat()),
            device("generic-16-pads", "16 pads (4×4)", 4, 4, pads(0.0, 0.0, 4, 4)),
            device("generic-16-pads-8-knobs", "16 pads + 8 knobs", 8, 4, [pads(0.0, 0.0, 4, 4), knobs(4.0, 0.0, 4, 2, true), transport(4.0, 2.0)].concat()),
            device("generic-64-pads", "8×8 pad grid", 8, 8, pads(0.0, 0.0, 8, 8)),
            device(
                "generic-25-keys",
                "25 keys + 8 pads + 8 knobs",
                9,
                4,
                [wheels(), pads(1.0, 0.0, 4, 2), knobs(5.0, 0.0, 4, 2, true), keys(0.0, 2.0, 9.0, 25)].concat(),
            ),
            device(
                "generic-49-keys",
                "49 keys + 9 faders + 8 knobs",
                11,
                5,
                [wheels(), knobs(1.0, 0.0, 8, 1, true), faders(1.0, 1.0, 9, 2.0), side_transport(), keys(0.0, 3.0, 11.0, 49)].concat(),
            ),
            device("generic-61-keys", "61 keys + 8 knobs", 12, 3, [wheels_short(), knobs(1.0, 0.0, 8, 1, true), side_transport_short(), keys(0.0, 1.0, 12.0, 61)].concat()),
        ]
    }

    /// `n` knobs, `per_row` across from `(x, y)`, CC 21 up; the first three
    /// are the surface's own dials when `dials`.
    fn knobs(x: f32, y: f32, per_row: u8, rows: u8, dials: bool) -> Vec<Control> {
        let n = per_row * rows;
        (0..n)
            .map(|i| {
                let k = control(format!("knob-{}", i + 1), format!("knob {}", i + 1), Part::Knob, at(x + (i % per_row) as f32, y + (i / per_row) as f32, 1.0, 1.0))
                    .sends(format!("cc:{}", 21 + i));
                match (dials, i) {
                    (true, 0) => k.plays("speed-dial"),
                    (true, 1) => k.plays("search-dial"),
                    (true, 2) => k.plays("fn-dial"),
                    _ => k,
                }
            })
            .collect()
    }

    fn faders(x: f32, y: f32, n: u8, h: f32) -> Vec<Control> {
        (0..n)
            .map(|i| control(format!("fader-{}", i + 1), format!("{}", i + 1), Part::Fader, at(x + i as f32, y, 1.0, h)).sends(format!("cc:{}", 41 + i)))
            .collect()
    }

    /// `cols × rows` pads from `(x, y)`, notes 36 up from the bottom-left.
    fn pads(x: f32, y: f32, cols: u8, rows: u8) -> Vec<Control> {
        let mut c = Vec::new();
        for row in 0..rows {
            for col in 0..cols {
                let n = 36 + (rows - 1 - row) as u16 * cols as u16 + col as u16;
                c.push(control(format!("pad-{}", row as u16 * cols as u16 + col as u16 + 1), "", Part::Pad, at(x + col as f32, y + row as f32, 1.0, 1.0)).sends(format!("note:{n}")));
            }
        }
        c
    }

    fn keys(x: f32, y: f32, w: f32, count: u8) -> Vec<Control> {
        let base = if count > 37 { 36 } else { 48 };
        vec![control("keys", "keys", Part::Keys { base, count }, at(x, y, w, 2.0))]
    }

    /// Pitch and mod wheels in the left cell, two rows tall.
    fn wheels() -> Vec<Control> {
        vec![
            control("pitch", "pitch", Part::Strip, at(0.0, 0.0, 0.5, 2.0)).sends("bend"),
            control("mod", "mod", Part::Strip, at(0.5, 0.0, 0.5, 2.0)).sends("cc:1"),
        ]
    }

    fn wheels_short() -> Vec<Control> {
        vec![
            control("pitch", "pitch", Part::Strip, at(0.0, 0.0, 0.5, 1.0)).sends("bend"),
            control("mod", "mod", Part::Strip, at(0.5, 0.0, 0.5, 1.0)).sends("cc:1"),
        ]
    }

    fn button(id: &str, label: &str, x: f32, y: f32, w: f32, h: f32, cc: u8, role: &str) -> Control {
        control(id, label, Part::Button, at(x, y, w, h)).sends(format!("cc:{cc}")).plays(role)
    }

    /// ◀ ▶ over ▶ ● in a 4 × 2 block.
    fn transport(x: f32, y: f32) -> Vec<Control> {
        vec![
            button("page-prev", "◀", x, y, 2.0, 1.0, 102, "page-prev"),
            button("page-next", "▶", x + 2.0, y, 2.0, 1.0, 103, "page-next"),
            button("play", "▶ play", x, y + 1.0, 2.0, 1.0, 115, "play"),
            button("record", "●", x + 2.0, y + 1.0, 2.0, 1.0, 117, "rec"),
        ]
    }

    /// ◀ ▶ ▶ ● stacked in the right cell, three rows tall.
    fn side_transport() -> Vec<Control> {
        vec![
            button("page-prev", "◀", 10.0, 0.0, 0.5, 1.0, 102, "page-prev"),
            button("page-next", "▶", 10.5, 0.0, 0.5, 1.0, 103, "page-next"),
            button("play", "▶ play", 10.0, 1.0, 1.0, 1.0, 115, "play"),
            button("record", "●", 10.0, 2.0, 1.0, 1.0, 117, "rec"),
        ]
    }

    fn side_transport_short() -> Vec<Control> {
        vec![
            button("page-prev", "◀", 9.0, 0.0, 0.5, 1.0, 102, "page-prev"),
            button("page-next", "▶", 9.5, 0.0, 0.5, 1.0, 103, "page-next"),
            button("play", "▶", 10.0, 0.0, 1.0, 1.0, 115, "play"),
            button("record", "●", 11.0, 0.0, 1.0, 1.0, 117, "rec"),
        ]
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
    fn the_catalogue_names_novation_akai_and_the_generic_layouts() {
        let all = catalogue();
        for maker in ["Novation", "Akai", "Korg", "Generic"] {
            assert!(all.iter().any(|d| d.maker == maker), "{maker}");
        }
        let mpk = device("mpk-mini-mk3").unwrap();
        assert_eq!(mpk.count(|p| matches!(p, Part::Pad)), 8);
        assert_eq!(mpk.count(|p| matches!(p, Part::Knob)), 8);
        assert!(for_port("MPK mini 3:MPK mini 3 MIDI 1 20:0").is_some_and(|d| d.id == "mpk-mini-mk3"));
        let mix = device("midimix").unwrap();
        assert_eq!(mix.count(|p| matches!(p, Part::Knob)), 24);
        assert_eq!(mix.count(|p| matches!(p, Part::Fader)), 9);
        assert_eq!(device("launch-control-xl").unwrap().count(|p| matches!(p, Part::Knob)), 24);
        let g49 = device("generic-49-keys").unwrap();
        assert!(g49.controls.iter().any(|c| c.part == Part::Keys { base: 36, count: 49 }));
        assert!(all.iter().filter(|d| d.maker == "Generic").all(|d| !d.is_port("anything")), "a generic layout is never a port");
        let pads = device("generic-16-pads").unwrap();
        let input = |id: &str| pads.controls.iter().find(|c| c.id == id).unwrap().input.clone().unwrap();
        assert_eq!((input("pad-13"), input("pad-4")), ("note:36".into(), "note:51".into()));
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
