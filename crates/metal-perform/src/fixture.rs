//! O.20 / S.AC — **fixture profiles as data**.
//!
//! A DMX fixture is a `Device` that drives a head instead of a screen. What
//! the head *is* — how many slots it takes, which slot carries pan, whether
//! pan has a fine channel, how much of the signal's signed travel it can
//! actually reach — is per model, so it lives in `data/fixtures/<model>.toml`
//! rather than in code. This module is the reader and the pure remap: signal
//! in, DMX slots out. It speaks no protocol ([`crate::output`]'s `out.*`
//! nodes own that) and it never touches the render loop.
//!
//! The rule the whole crate repeats applies here too: **a fixture profile is
//! data, and a missing or malformed file is a skip, never a crash.** A show
//! does not stop because a probe arrived with a typo in its TOML.
//!
//! See `plan-opus.md` § O.20, decision 11 (`plan-opus.md`): the pan/tilt
//! floats of DMX are their own, they do not mount on a model's float lanes.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use metal_schema::Signal;

/// Which float of the [`Signal`] a channel carries.
///
/// The names match the row's own `sig_*` floats (decision: the profile learns
/// nothing the row does not already say). `sig_a`/`sig_b` are reachable by
/// their short names only — they are deliberately unnamed in the schema.
///
/// **Appended at the end** (`Cct`, then the four beam-shaping channels,
/// 2026-09-27): a profile is read by name, but the enum is `Serialize`, so a
/// saved config that spelled `"a"` still round-trips.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Selector {
    Level,
    Hue,
    Sat,
    Strobe,
    Pan,
    Tilt,
    A,
    B,
    Cct,
    Zoom,
    Focus,
    Iris,
    Frost,
}

impl Selector {
    /// The row float this selector reads, in the row's own units.
    pub fn read(self, signal: &Signal) -> f32 {
        match self {
            Selector::Level => signal.level,
            Selector::Hue => signal.hue,
            Selector::Sat => signal.sat,
            Selector::Strobe => signal.strobe,
            Selector::Pan => signal.pan,
            Selector::Tilt => signal.tilt,
            Selector::A => signal.a,
            Selector::B => signal.b,
            Selector::Cct => signal.cct,
            Selector::Zoom => signal.zoom,
            Selector::Focus => signal.focus,
            Selector::Iris => signal.iris,
            Selector::Frost => signal.frost,
        }
    }

    /// The float's declared range, so a profile that omits `range` still
    /// remaps correctly. Mirrors [`metal_schema::SIGNAL_FLOATS`].
    pub fn default_range(self) -> [f32; 2] {
        match self {
            Selector::Level
            | Selector::Hue
            | Selector::Sat
            | Selector::A
            | Selector::B
            | Selector::Zoom
            | Selector::Focus
            | Selector::Iris
            | Selector::Frost => [0.0, 1.0],
            Selector::Strobe => [0.0, 24.0],
            Selector::Pan | Selector::Tilt => [-1.0, 1.0],
            Selector::Cct => [1800.0, 10000.0],
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Selector::Level => "level",
            Selector::Hue => "hue",
            Selector::Sat => "sat",
            Selector::Strobe => "strobe",
            Selector::Pan => "pan",
            Selector::Tilt => "tilt",
            Selector::A => "a",
            Selector::B => "b",
            Selector::Cct => "cct",
            Selector::Zoom => "zoom",
            Selector::Focus => "focus",
            Selector::Iris => "iris",
            Selector::Frost => "frost",
        }
    }

    /// Parses `level`, `pan`, `sig_tilt`, … — the `sig_` prefix is accepted
    /// because that is how the row and the schema spell it, but not required.
    pub fn parse(text: &str) -> Option<Selector> {
        let name = text.strip_prefix("sig_").unwrap_or(text);
        Some(match name {
            "level" => Selector::Level,
            "hue" => Selector::Hue,
            "sat" => Selector::Sat,
            "strobe" => Selector::Strobe,
            "pan" => Selector::Pan,
            "tilt" => Selector::Tilt,
            "a" => Selector::A,
            "b" => Selector::B,
            "cct" => Selector::Cct,
            "zoom" => Selector::Zoom,
            "focus" => Selector::Focus,
            "iris" => Selector::Iris,
            "frost" => Selector::Frost,
            _ => return None,
        })
    }
}

/// One head channel: which signal it carries, which DMX slots it occupies,
/// and the two ranges that turn a float into a byte.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Channel {
    pub signal: Selector,
    /// The slot the coarse (high) byte lands in. Slots are 0-based here; the
    /// profile's `address` is 1-based, the way a desk numbers them.
    ///
    /// `None` for a `wire = "bluetooth"` profile: a Bluetooth fixture has no
    /// DMX slots at all, so there is no coarse byte to place. A DMX profile
    /// without one is refused at parse ([`parse`]), which is the behaviour
    /// `data/fixtures/*.toml` always relied on.
    #[serde(default)]
    pub coarse: Option<u16>,
    /// The slot for the fine (low) byte, when the head has one. A 16-bit
    /// channel is coarse + fine; an 8-bit channel omits it.
    #[serde(default)]
    pub fine: Option<u16>,
    /// The slice of the signal float's own units that maps to full travel.
    /// Omitted means the float's declared range ([`Selector::default_range`]).
    /// A head that only tilts ±30° declares `[-0.5, 0.5]` and the rest of the
    /// signal is clamped, rather than silently compressed.
    #[serde(default)]
    pub range: Option<[f32; 2]>,
    /// The raw slot range. Omitted means `0..255`, or `0..65535` when a fine
    /// slot exists.
    #[serde(default)]
    pub physical: Option<[f32; 2]>,
}

impl Selector {
    /// Whether `PatchMode::Patch` owns this signal: the channels that move or
    /// shape the beam. Everything else was always packed, so `Colour` mode (the
    /// default) behaves exactly as every venue did before this existed.
    pub fn patch_owned(self) -> bool {
        matches!(
            self,
            Selector::Pan | Selector::Tilt | Selector::Zoom | Selector::Focus | Selector::Iris | Selector::Frost
        )
    }
}

impl Channel {
    pub fn signal_range(&self) -> [f32; 2] {
        self.range.unwrap_or_else(|| self.signal.default_range())
    }

    pub fn physical_range(&self) -> [f32; 2] {
        self.physical.unwrap_or(if self.fine.is_some() {
            [0.0, 65535.0]
        } else {
            [0.0, 255.0]
        })
    }

    /// The raw 16-bit (or 8-bit) value this channel asks for, as one number.
    /// `physical` is the slot range, so a 16-bit channel returns 0..65535.
    pub fn raw(&self, signal: &Signal) -> f32 {
        remap(
            self.signal.read(signal),
            self.signal_range(),
            self.physical_range(),
        )
    }

    /// The bytes this channel writes, as `(slot, byte)` pairs — one for a
    /// coarse channel, two for a 16-bit one. Order within the pair is coarse
    /// then fine, matching how a desk packs them.
    ///
    /// Empty for a Bluetooth channel ([`Channel::coarse`] `None`): there is
    /// no slot to write, and the profile's [`Fixture::light_state`] is the
    /// path that reaches such a fixture instead.
    pub fn bytes(&self, signal: &Signal) -> Vec<(u16, u8)> {
        let Some(coarse_slot) = self.coarse else {
            return Vec::new();
        };
        let value = self.raw(signal).round().clamp(0.0, 65535.0) as u32;
        match self.fine {
            Some(fine) => {
                let coarse = ((value >> 8) & 0xff) as u8;
                let low = (value & 0xff) as u8;
                vec![(coarse_slot, coarse), (fine, low)]
            }
            None => vec![(coarse_slot, value.min(255) as u8)],
        }
    }
}

/// Pure linear remap with clamp — the one function every channel shares.
///
/// Exposed because it is the whole of the deterministic-fallback story: given
/// the same signal and the same profile, the show is the same frame after
/// frame, machine after machine.
pub fn remap(value: f32, from: [f32; 2], to: [f32; 2]) -> f32 {
    let span = from[1] - from[0];
    let t = if span.abs() < f32::EPSILON {
        0.0
    } else {
        ((value - from[0]) / span).clamp(0.0, 1.0)
    };
    to[0] + t * (to[1] - to[0])
}

/// How a fixture is reached. `Dmx` is the default and the only thing a
/// `data/fixtures/*.toml` profile said before Bluetooth existed; `Bluetooth`
/// is a Godox-class lamp addressed by profile name over `metal-ble`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WireKind {
    #[default]
    Dmx,
    Bluetooth,
}

/// The `[bluetooth]` table of a `wire = "bluetooth"` profile. `protocol` names
/// the wire protocol ([`metal_ble::Protocol`]'s serde spelling, `"godox-mesh"`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Bluetooth {
    pub protocol: metal_ble::Protocol,
}

/// A pan/tilt no-go zone (M6): a rectangle of the head's travel it must not
/// enter — a pillar in the way, the audience's eyes. Angles in the same world
/// degrees as `pan`/`tilt`; a pose inside the rectangle is pushed to the
/// nearest edge.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NoGo {
    #[serde(default = "default_pan")]
    pub pan: [f32; 2],
    #[serde(default = "default_tilt")]
    pub tilt: [f32; 2],
}

/// A fixture's physical limits and safety envelope, as a profile states them
/// (M1, M6 of the pixel map). Every field has a serde default, so a profile
/// written before the table existed still parses and a head that says nothing
/// still moves.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Limits {
    /// Pan travel, degrees, absolute — a 540° head is `[-270, 270]`.
    #[serde(default = "default_pan")]
    pub pan: [f32; 2],
    /// Tilt travel, degrees — a 270° head is `[-135, 135]`.
    #[serde(default = "default_tilt")]
    pub tilt: [f32; 2],
    /// How fast the head slews, degrees per second.
    #[serde(default = "default_speed")]
    pub speed: f32,
    /// Gobo/rotation travel, degrees.
    #[serde(default = "default_rotation")]
    pub rotation: [f32; 2],
    /// Zoom travel, degrees of beam angle.
    #[serde(default = "default_zoom")]
    pub zoom: [f32; 2],
    /// Dimmer travel, 0..1.
    #[serde(default = "unit_range")]
    pub dimmer: [f32; 2],
    /// Colour temperature travel, kelvin.
    #[serde(default = "default_cct")]
    pub cct: [f32; 2],
    /// The brightest the fixture may be driven, `0..1` (M6). The state clamps
    /// to it before the wire, so a profile can cap a fixture that is too
    /// bright for the room.
    #[serde(default = "unit")]
    pub max_dimmer: f32,
    /// The profile refuses strobe: the state zeroes it before the wire (M6).
    #[serde(default)]
    pub no_strobe: bool,
    /// Pan/tilt no-go zones (M6): a head aimed into one is pushed to the
    /// nearest edge.
    #[serde(default)]
    pub no_go: Vec<NoGo>,
}

impl Default for Limits {
    fn default() -> Limits {
        Limits {
            pan: default_pan(),
            tilt: default_tilt(),
            speed: default_speed(),
            rotation: default_rotation(),
            zoom: default_zoom(),
            dimmer: unit_range(),
            cct: default_cct(),
            max_dimmer: unit(),
            no_strobe: false,
            no_go: Vec::new(),
        }
    }
}

impl Limits {
    /// Clamp a pan/tilt pair into the travel and out of every no-go zone
    /// (M6). Returns the pair a head may actually hold.
    pub fn clamp_pose(&self, pan: f32, tilt: f32) -> (f32, f32) {
        let (lo, hi) = (self.pan[0].min(self.pan[1]), self.pan[0].max(self.pan[1]));
        let (tlo, thi) = (self.tilt[0].min(self.tilt[1]), self.tilt[0].max(self.tilt[1]));
        let mut p = pan.clamp(lo, hi);
        let mut t = tilt.clamp(tlo, thi);
        for z in &self.no_go {
            let (zlo, zhi) = (z.pan[0].min(z.pan[1]), z.pan[0].max(z.pan[1]));
            let (z0, z1) = (z.tilt[0].min(z.tilt[1]), z.tilt[0].max(z.tilt[1]));
            if p > zlo && p < zhi && t > z0 && t < z1 {
                // Push out along whichever edge is nearest, pan first.
                let dp = (p - zlo).min(zhi - p);
                let dt = (t - z0).min(z1 - t);
                if dp <= dt {
                    p = if p - zlo <= zhi - p { zlo } else { zhi };
                } else {
                    t = if t - z0 <= z1 - t { z0 } else { z1 };
                }
            }
        }
        (p, t)
    }

    /// Clamp a signal to the profile's safety envelope before it reaches the
    /// wire (M6): the dimmer capped at `max_dimmer`, the strobe zeroed when
    /// the profile refuses it, and pan/tilt pushed out of the no-go zones.
    pub fn clamp_signal(&self, s: &mut metal_schema::Signal) {
        s.level = s.level.clamp(self.dimmer[0].min(self.dimmer[1]), self.max_dimmer.clamp(0.0, 1.0));
        if self.no_strobe {
            s.strobe = 0.0;
        }
        let (pan, tilt) = self.clamp_pose(s.pan, s.tilt);
        s.pan = pan;
        s.tilt = tilt;
    }
}

fn default_pan() -> [f32; 2] {
    [-270.0, 270.0]
}
fn default_tilt() -> [f32; 2] {
    [-135.0, 135.0]
}
fn default_speed() -> f32 {
    180.0
}
fn default_rotation() -> [f32; 2] {
    [0.0, 360.0]
}
fn default_zoom() -> [f32; 2] {
    [4.0, 60.0]
}
fn unit_range() -> [f32; 2] {
    [0.0, 1.0]
}
fn unit() -> f32 {
    1.0
}
fn default_cct() -> [f32; 2] {
    [1800.0, 10000.0]
}

/// A fixture model, as read from `data/fixtures/<model>.toml`.
#[derive(Debug, Clone, PartialEq)]
pub struct Fixture {
    /// The file stem — the id a [`Head`] names.
    pub model: String,
    pub label: String,
    /// Which wire reaches this fixture. `Dmx` unless the profile says
    /// `wire = "bluetooth"`.
    pub wire: WireKind,
    /// Present exactly when `wire == Bluetooth`.
    pub bluetooth: Option<Bluetooth>,
    /// The DMX universe this model defaults to. Meaningless for a Bluetooth
    /// profile (which writes no slots) and left at its default there.
    pub universe: u16,
    /// The 1-based start address this model defaults to. A [`Head`] may
    /// override it (two probes of the same model side by side). Meaningless
    /// for a Bluetooth profile.
    pub address: u16,
    pub channels: Vec<Channel>,
    /// The head's physical limits (M1). Defaults when the profile omits the
    /// `[limits]` table.
    pub limits: Limits,
}

/// One device's head: which fixture it is, and the address it sits at.
/// `universe`/`address` omitted mean the profile's own defaults.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Head {
    pub fixture: String,
    #[serde(default)]
    pub universe: Option<u16>,
    #[serde(default)]
    pub address: Option<u16>,
}

impl Fixture {
    /// The universe a head of this fixture occupies.
    pub fn universe_of(&self, head: &Head) -> u16 {
        head.universe.unwrap_or(self.universe)
    }

    /// The 1-based address a head of this fixture starts at.
    pub fn address_of(&self, head: &Head) -> u16 {
        head.address.unwrap_or(self.address)
    }

    /// Every slot this fixture writes, as absolute 1-based DMX addresses, in
    /// slot order. Used to place a head on a universe without overlap. A
    /// Bluetooth profile writes no slots, so this is empty for one.
    pub fn addresses(&self, head: &Head) -> Vec<u16> {
        let base = self.address_of(head);
        let mut out: Vec<u16> = self
            .channels
            .iter()
            .flat_map(|c| {
                c.coarse
                    .into_iter()
                    .chain(c.fine)
                    .map(move |rele| base + rele)
            })
            .collect();
        out.sort_unstable();
        out
    }

    /// The bytes this head writes for a signal, as `(absolute 1-based
    /// address, byte)` pairs, sorted by address. This is what an `out.*` node
    /// would copy into its universe.
    pub fn frame(&self, head: &Head, signal: &Signal) -> Vec<(u16, u8)> {
        let base = self.address_of(head);
        let mut out: Vec<(u16, u8)> = self
            .channels
            .iter()
            .flat_map(|c| c.bytes(signal).into_iter())
            .map(|(slot, byte)| (base + slot, byte))
            .collect();
        out.sort_by_key(|(address, _)| *address);
        out
    }
    /// [`Fixture::frame`] with an explicit [`crate::wire::PatchMode`]. Both
    /// modes write every channel the profile names, so a fixture's channel
    /// count never changes; `Colour` writes the pan/tilt and beam-shaping
    /// channels at their untouched value, `Patch` writes them from `signal`.
    pub fn frame_mode(
        &self,
        head: &Head,
        signal: &Signal,
        patch: crate::wire::PatchMode,
    ) -> Vec<(u16, u8)> {
        let base = self.address_of(head);
        let held = Signal {
            level: 0.0,
            hue: 0.0,
            sat: 0.0,
            strobe: 0.0,
            pan: 0.0,
            tilt: 0.0,
            a: 0.0,
            b: 0.0,
            cct: 5600.0,
            zoom: 0.0,
            focus: 0.0,
            iris: 0.0,
            frost: 0.0,
        };
        let mut out: Vec<(u16, u8)> = self
            .channels
            .iter()
            .flat_map(|c| {
                let from = if patch == crate::wire::PatchMode::Patch || !c.signal.patch_owned() {
                    signal
                } else {
                    &held
                };
                c.bytes(from).into_iter()
            })
            .map(|(slot, byte)| (base + slot, byte))
            .collect();
        out.sort_by_key(|(address, _)| *address);
        out
    }

    /// The profile as the `[[channel]]` TOML `parse` reads back — the write
    /// side of a profile, used by the GDTF importer to land a `.gdtf` as
    /// `data/fixtures/<model>.toml`. `model` is the file stem (not a field),
    /// so it is not serialized; `universe`/`address`/`limits` write with
    /// their defaults so a re-read is a no-op.
    pub fn to_toml(&self) -> Result<String, String> {
        #[derive(Serialize)]
        struct Profile<'a> {
            label: &'a str,
            wire: WireKind,
            universe: u16,
            address: u16,
            channel: &'a [Channel],
            limits: &'a Limits,
        }
        let profile = Profile {
            label: &self.label,
            wire: self.wire,
            universe: self.universe,
            address: self.address,
            channel: &self.channels,
            limits: &self.limits,
        };
        toml::to_string_pretty(&profile).map_err(|e| e.to_string())
    }

    /// What a Bluetooth fixture is asked for, as the [`metal_ble::LightState`]
    /// `metal-ble`'s link pushes — the Bluetooth counterpart of [`Fixture::frame`].
    ///
    /// The mapping is the one the plan fixes: `level` is remapped through the
    /// profile's `level` channel `range` (a lamp whose support tops out at
    /// 60% of the row still runs its own full travel), `cct_kelvin` is `Some`
    /// only when the profile actually has a `cct` channel (so a daylight-only
    /// SL60II D is never handed a temperature it would refuse), and `hsi` is
    /// `Some` when the profile has both `hue` and `sat` and the saturation is
    /// above zero (`sat` 0 is white, which is `cct`'s job, not HSI's — see the
    /// `sat` 0 note in [`metal_schema::Signal::rgb`]).
    ///
    /// A profile that names no `level` channel at all is treated as level 0:
    /// a fixture with no master float is off, not a guess.
    pub fn light_state(&self, s: &Signal) -> metal_ble::LightState {
        let level = self
            .channels
            .iter()
            .find(|c| c.signal == Selector::Level)
            .map(|c| remap(s.level, c.signal_range(), [0.0, 1.0]))
            .unwrap_or(0.0)
            .clamp(0.0, 1.0);

        let cct_kelvin = self
            .channels
            .iter()
            .find(|c| c.signal == Selector::Cct)
            .map(|c| {
                // The signal's own units *are* kelvin, so the profile's
                // `range` is the window the lamp supports, not a byte travel:
                // clamp into it rather than remap a byte out of it. A row
                // asking for 9000 K on an SL60II Bi stops at 6500 K.
                let [lo, hi] = c.signal_range();
                s.cct.clamp(lo.min(hi), hi.max(lo))
            });

        let has = |sel: Selector| self.channels.iter().any(|c| c.signal == sel);
        let hsi = if has(Selector::Hue) && has(Selector::Sat) && s.sat > 0.0 {
            Some(metal_ble::Hsi {
                hue_deg: s.hue.rem_euclid(1.0) * 360.0,
                sat: s.sat.clamp(0.0, 1.0),
            })
        } else {
            None
        };

        metal_ble::LightState { level, cct_kelvin, hsi }
    }

    /// F6: the selectors this profile declares, in the canonical order the
    /// shell's fixture page lists them (`level`, `hue`, `sat`, `strobe`,
    /// `pan`, `tilt`, `cct`, `zoom`, `focus`, `iris`, `frost`), deduplicated.
    /// `a`/`b` are read (`Selector::read`) but are not page columns; they are
    /// reported here anyway so a caller can decide.
    pub fn selectors(&self) -> Vec<Selector> {
        let order = [
            Selector::Level,
            Selector::Hue,
            Selector::Sat,
            Selector::Strobe,
            Selector::Pan,
            Selector::Tilt,
            Selector::Cct,
            Selector::Zoom,
            Selector::Focus,
            Selector::Iris,
            Selector::Frost,
            Selector::A,
            Selector::B,
        ];
        order
            .into_iter()
            .filter(|s| self.channels.iter().any(|c| c.signal == *s))
            .collect()
    }
}

// ---------------------------------------------------------------------------
// loading
// ---------------------------------------------------------------------------

/// The directory the profiles live in, relative to the data root.
pub const DIR: &str = "fixtures";

pub fn dir(root: &Path) -> PathBuf {
    root.join(DIR)
}

/// The on-disk shape. `model` is the file stem, not a field; `channel` is a
/// TOML array of tables.
#[derive(Debug, Deserialize)]
struct Raw {
    #[serde(default)]
    label: Option<String>,
    #[serde(default)]
    wire: WireKind,
    #[serde(default)]
    bluetooth: Option<Bluetooth>,
    #[serde(default)]
    universe: Option<u16>,
    #[serde(default = "one")]
    address: u16,
    #[serde(default)]
    channel: Vec<Channel>,
    #[serde(default)]
    limits: Limits,
}

fn one() -> u16 {
    1
}

/// Parses one profile. Returns an error for anything that would make the
/// bytes ambiguous: no channels, a signal named twice, a slot read twice, a
/// fine slot without a matching coarse, or a degenerate range.
///
/// The wire decides what a channel may omit. A **DMX** profile must give
/// every channel a `coarse` slot — that has always been true and still is,
/// so a profile that forgot one is refused rather than silently sending
/// nothing. A **Bluetooth** profile must *not*: it has no slots, and a
/// `coarse`/`fine` in one would be a channel claiming a byte it never writes.
pub fn parse(model: &str, text: &str) -> Result<Fixture, String> {
    let raw: Raw = toml::from_str(text).map_err(|e| e.to_string())?;
    if raw.channel.is_empty() {
        return Err(format!("{model}: no [[channel]] entries"));
    }
    if raw.wire == WireKind::Bluetooth && raw.bluetooth.is_none() {
        return Err(format!("{model}: wire = \"bluetooth\" needs a [bluetooth] table"));
    }
    if raw.wire == WireKind::Dmx && raw.bluetooth.is_some() {
        return Err(format!("{model}: [bluetooth] is set but wire is not \"bluetooth\""));
    }

    let mut signals: Vec<Selector> = Vec::new();
    let mut slots: Vec<u16> = Vec::new();
    for (i, channel) in raw.channel.iter().enumerate() {
        if signals.contains(&channel.signal) {
            return Err(format!(
                "{model}: channel {i} repeats signal `{}`",
                channel.signal.name()
            ));
        }
        signals.push(channel.signal);

        match (raw.wire, channel.coarse) {
            (WireKind::Dmx, None) => {
                return Err(format!("{model}: channel {i} has no coarse slot (DMX profile)"));
            }
            (WireKind::Bluetooth, Some(_)) => {
                return Err(format!(
                    "{model}: channel {i} has a coarse slot on a Bluetooth profile"
                ));
            }
            _ => {}
        }

        let mut here: Vec<u16> = channel.coarse.into_iter().collect();
        if let Some(fine) = channel.fine {
            if channel.coarse == Some(fine) {
                return Err(format!(
                    "{model}: channel {i} has the same coarse and fine slot"
                ));
            }
            here.push(fine);
        }
        for slot in &here {
            if slots.contains(slot) {
                return Err(format!("{model}: slot {slot} is written twice"));
            }
            slots.push(*slot);
        }

        if let Some([a, b]) = channel.range {
            if (b - a).abs() < f32::EPSILON {
                return Err(format!("{model}: channel {i} has an empty range"));
            }
        }
    }

    Ok(Fixture {
        model: model.to_string(),
        label: raw.label.unwrap_or_else(|| model.to_string()),
        wire: raw.wire,
        bluetooth: raw.bluetooth,
        universe: raw.universe.unwrap_or(0),
        address: raw.address,
        channels: raw.channel,
        limits: raw.limits,
    })
}

/// Every profile under `data/fixtures/`, sorted by model. A missing directory
/// is an empty list; a file that fails to parse is skipped with a line on
/// stderr, never a crash.
pub fn load(root: &Path) -> Vec<Fixture> {
    let mut out: Vec<Fixture> = Vec::new();
    let Ok(entries) = metal_vfs::read_dir(dir(root)) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("toml") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        match metal_vfs::read_to_string(&path) {
            Ok(text) => match parse(stem, &text) {
                Ok(fixture) => out.push(fixture),
                Err(e) => eprintln!("  fixture {stem} skipped: {e}"),
            },
            Err(e) => eprintln!("  fixture {stem} unreadable: {e}"),
        }
    }
    out.sort_by(|a, b| a.model.cmp(&b.model));
    out
}

/// One profile by model id.
pub fn get(root: &Path, model: &str) -> Option<Fixture> {
    load(root).into_iter().find(|f| f.model == model)
}

// ---------------------------------------------------------------------------
// deterministic fallback
// ---------------------------------------------------------------------------

/// The **deterministic fallback** of O.20: a digest signature in, a row of
/// head signals out, with no model and no training in between. Same
/// signature, same signal — the property the hotel test leaned on.
///
/// Deliberately plain: the first eight signature columns drive the eight
/// selectors in a fixed order, wrapped when the signature is shorter. It is a
/// fallback a show can trust, not a look.
pub fn fallback_signals(signature: &[f32], heads: usize) -> Vec<Signal> {
    if heads == 0 {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(heads);
    for head in 0..heads {
        let at = |i: usize| -> f32 {
            if signature.is_empty() {
                0.0
            } else {
                let v = signature[(head + i) % signature.len()];
                if v.is_finite() {
                    v
                } else {
                    0.0
                }
            }
        };
        out.push(Signal {
            level: at(0).clamp(0.0, 1.0),
            hue: at(1).rem_euclid(1.0),
            sat: at(2).clamp(0.0, 1.0),
            strobe: (at(3).abs() * 24.0).clamp(0.0, 24.0),
            pan: at(4).clamp(-1.0, 1.0),
            tilt: at(5).clamp(-1.0, 1.0),
            a: at(6).clamp(0.0, 1.0),
            b: at(7).clamp(0.0, 1.0),
            // The deterministic fallback still drives the original eight
            // selectors by the signature's own columns; the new floats sit at
            // the schema's defaults rather than borrowing a column, so a
            // signature shorter than thirteen keeps meaning what it meant.
            cct: metal_schema::SIGNAL_FLOATS[8].default,
            zoom: metal_schema::SIGNAL_FLOATS[9].default,
            focus: metal_schema::SIGNAL_FLOATS[10].default,
            iris: metal_schema::SIGNAL_FLOATS[11].default,
            frost: metal_schema::SIGNAL_FLOATS[12].default,
        });
    }
    out
}


// ---------------------------------------------------------------------------
// S.O.1 — `FixtureConfig`: "explicit black" on an `out.*` node.
// `S.O.1`: "explicit black" on an `out.*` fixture node — the half `bypass`
// does not reach. `plan-opus.md § O.28` point 5: blackout is `bypass` of
// every `output.device` **plus** black on every `out.*`, because a
// bypassed fixture is not necessarily a *black* fixture — `bypass` only
// tells `compile`/`compose` to skip rendering a node, and says nothing
// about the last DMX/sACN/… values already latched on the real hardware.
// Blackout needs a second, explicit fact recorded on the node itself:
// "drive this fixture black regardless of whatever is cabled into it".
//
// **Same shape as [`crate::node_layer::NodeLayerConfig`].** An optional
// field, read out of [`metal_graph::Node::config`], that overrides nothing
// when absent — the same "everything here is optional, and that is the
// whole trick" discipline that file's own doc states.
//
// **Minimal on purpose.** This lote only needs "force black", not a
// general "force any color" surface — but `forced` is typed as the color
// blackout wants (`[0.0, 0.0, 0.0]`) rather than a bare `bool` with black
// hardcoded elsewhere, because the next thing that wants "force this
// fixture to a color" (a solid-color overlay reaching fixtures, say) gets
// it for free instead of inventing a second field that means the same
// thing. Nothing beyond that one field is built speculatively.
//
// **Where `forced` is read.** No `out.*` kind has a real *wire* backend yet
// — nothing opens a socket, `out.sacn` included, see `plan-sonnet.md §
// S.O.3` — but the slots a head would be sent are computed here, and that
// is where `forced` is honoured: [`FixtureConfig::drive`] rewrites the
// wire's own [`Signal`] before [`Fixture::frame`] packs it, and
// [`drive_head`] is the one call a backend makes per frame, reading the
// config fresh off the device's graph ([`forced_on`], regla 4) rather than
// off any cached bit. The day a socket lands, it sends what `drive_head`
// answers; nothing about blackout changes.

/// What an `out.*` fixture node can force, read straight out of
/// [`metal_graph::Node::config`]. `forced: None` means "nothing pinned — draw
/// whatever the wire would otherwise carry".
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FixtureConfig {
    #[serde(default)]
    pub forced: Option<[f32; 3]>,
    /// Which channels a `venue.fixture` node packs: colour only (the
    /// default, every existing caller) or the whole signal block. See
    /// [`crate::wire::PatchMode`].
    #[serde(default)]
    pub patch: crate::wire::PatchMode,
}

impl FixtureConfig {
    /// Reads whatever a node's free-form `config` holds as a
    /// `FixtureConfig`, treating anything that does not parse as one — an
    /// empty config, an unrelated node's own shape, garbage from a
    /// hand-edited file — as "nothing forced" rather than a hard error,
    /// the same fallback `NodeLayerConfig::from_node` takes.
    pub fn from_node(node: &metal_graph::Node) -> FixtureConfig {
        if node.config.is_empty() {
            return FixtureConfig::default();
        }
        serde_json::from_str(&node.config.to_json_str()).unwrap_or_default()
    }

    pub fn to_config(&self) -> metal_graph::Config {
        // unwrap: `FixtureConfig` only ever contains an `Option<[f32; 3]>`,
        // which always round-trips through serde_json.
        let s = serde_json::to_string(self).expect("FixtureConfig always serializes");
        metal_graph::Config::from_json_str(&s).expect("just-serialized JSON always parses")
    }

    pub fn is_empty(&self) -> bool {
        *self == FixtureConfig::default()
    }

    /// The signal a head is actually sent: `forced` wins over the wire.
    ///
    /// A forced colour replaces the three floats a colour *is* on a head —
    /// `level`, `hue`, `sat` — and zeroes `strobe`, because a fixture pinned
    /// to black that keeps strobing is not black. `pan`/`tilt`/`a`/`b` are
    /// left as the wire had them: blackout switches a light off, it does
    /// not swing the head somewhere else, and releasing it must find the
    /// beam where the show left it. `forced: None` is the wire, untouched.
    pub fn drive(&self, wire: &Signal) -> Signal {
        let Some(rgb) = self.forced else {
            return *wire;
        };
        let (level, hue, sat) = level_hue_sat(rgb);
        Signal {
            level,
            hue,
            sat,
            strobe: 0.0,
            ..*wire
        }
    }
}

/// Writes `forced` onto an `out.*` node's config — or, with `None`, takes it
/// off — keeping every other key the node has: an `out.sacn`'s `universe`
/// and `unicast`, a frame-driven node's `heads`/`sample`, an `out.serial`'s
/// `port`. What `POST /overlay/blackout` writes onto every fixture: replacing
/// the whole config with a bare [`FixtureConfig`] would wipe the wire's own
/// settings on the way in and never bring them back on release. Releasing
/// leaves the config exactly as it was before the blackout.
pub fn set_forced(node: &mut metal_graph::Node, forced: Option<[f32; 3]>) {
    let mut cfg = serde_json::from_str::<serde_json::Value>(&node.config.to_json_str())
        .ok()
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default();
    match forced {
        Some(rgb) => {
            cfg.insert("forced".to_string(), serde_json::json!(rgb));
        }
        None => {
            cfg.remove("forced");
        }
    }
    node.config = if cfg.is_empty() {
        metal_graph::Config::default()
    } else {
        // unwrap: a `serde_json` object always serializes to JSON that parses.
        metal_graph::Config::from_json_str(&serde_json::Value::Object(cfg).to_string())
            .expect("a JSON object always parses")
    };
}

/// `level`/`hue`/`sat` of an `[r, g, b]` in 0..1 — the three floats a head
/// reads a colour as ([`Selector::Level`]/[`Selector::Hue`]/[`Selector::Sat`]).
/// Plain HSV, with `level` the max channel: black is `(0, 0, 0)`, a pure red
/// `(1, 0, 1)`. Inputs outside 0..1 are clamped, not wrapped.
fn level_hue_sat(rgb: [f32; 3]) -> (f32, f32, f32) {
    let [r, g, b] = rgb.map(|v| if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 });
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let delta = max - min;
    let sat = if max > 0.0 { delta / max } else { 0.0 };
    let hue = if delta <= 0.0 {
        0.0
    } else if max == r {
        ((g - b) / delta).rem_euclid(6.0) / 6.0
    } else if max == g {
        ((b - r) / delta + 2.0) / 6.0
    } else {
        ((r - g) / delta + 4.0) / 6.0
    };
    (max, hue, sat)
}

/// The colour a device's graph forces onto its fixtures right now, if any:
/// the first `out.*` node carrying a [`FixtureConfig::forced`]. Read off
/// the graph every call — this is the fixture half of `GET /overlay/blackout`'s
/// "fresh off the graphs" rule, and the reason a blackout released by hand
/// in the canvas is seen on the next frame.
pub fn forced_on(graph: &metal_graph::Graph) -> Option<[f32; 3]> {
    graph
        .nodes
        .iter()
        .filter(|n| n.kind.starts_with("out."))
        .find_map(|n| FixtureConfig::from_node(n).forced)
}

/// The slots one head is sent this frame, as `(absolute 1-based address,
/// byte)` pairs — the one call a wire backend makes per device per frame.
///
/// `wire` is what the show is asking for (a layer row's `sig_*` block,
/// `metal_schema::read_signal`). Two things win over it, in this order, both
/// read at the call and never cached: the device's authored `graph`, whose
/// `out.*` nodes may pin a colour ([`forced_on`] — `POST /overlay/blackout`'s
/// fixture half), and `cover`, the colour a timed overlay has over the whole
/// device right now (`crate::overlay::Overlays::cover` — a solid for N
/// seconds, a timed blackout — the same value the screen is painted with,
/// so a room on red is red on the heads too). `None` when the head names a
/// profile that is not under `data/fixtures/` — the "a missing profile is
/// a skip, never a crash" rule at the top of this file, applied to the
/// render path.
pub fn drive_head(
    root: &Path,
    head: &Head,
    graph: Option<&metal_graph::Graph>,
    cover: Option<[u8; 3]>,
    wire: &Signal,
) -> Option<Vec<(u16, u8)>> {
    drive_head_mode(root, head, graph, cover, wire, crate::wire::PatchMode::Colour)
}

/// [`drive_head`] with an explicit [`crate::wire::PatchMode`]. `Patch` packs
/// the row's pan/tilt and beam shaping as well as its colour, so one graph row
/// can aim a head and light it (the hotel-fiesta rule).
pub fn drive_head_mode(
    root: &Path,
    head: &Head,
    graph: Option<&metal_graph::Graph>,
    cover: Option<[u8; 3]>,
    wire: &Signal,
    patch: crate::wire::PatchMode,
) -> Option<Vec<(u16, u8)>> {
    let fixture = get(root, &head.fixture)?;
    let cfg = FixtureConfig {
        forced: graph
            .and_then(forced_on)
            .or_else(|| cover.map(|c| c.map(|v| v as f32 / 255.0))),
        patch,
    };
    Some(fixture.frame_mode(head, &cfg.drive(wire), patch))
}

/// The slots a `venue.fixture` node's head is sent this frame from a
/// [`Signal`] — the `signal` colour source (decision 3, F3), and the shape
/// the model source (F4) lands in too. Packs through [`drive_head`], so the
/// graph's blackout (`forced`) and the device's cover colour win over the wire
/// exactly as on an `out.*`: `level`/`hue`/`sat` and the strobe go, `pan`/
/// `tilt` and the beam shaping stay where the wire put them.
///
/// The node's config is a pointer: the row is looked up by `fixture_id`, and
/// its profile, universe and address are used as the [`Head`]'s (only when the
/// row overrides them — otherwise the profile's own defaults apply, the same
/// reading [`crate::wire::venue_heads`] takes). `None` when the row or its
/// profile is missing: an orphan is a skip, never a crash and never a send
/// (decision 1).
pub fn drive_venue_fixture_signal(
    root: &Path,
    venue: &crate::venue::Venue,
    fixture_id: &str,
    graph: Option<&metal_graph::Graph>,
    cover: Option<[u8; 3]>,
    wire: &Signal,
) -> Option<Vec<(u16, u8)>> {
    let f = venue.fixtures.iter().find(|f| f.id == fixture_id)?;
    let head = Head { fixture: f.profile.clone()?, universe: f.universe, address: f.address };
    drive_head(root, &head, graph, cover, wire)
}

/// A colour's `hue`/`sat`/`level` floats (each `0..1`, [`crate::venue::light_floats`])
/// with the `hue_lock` decision applied (F4, decision 6): `hold` swaps the
/// driven hue for `own`'s — the source model's own base colour in the same
/// slot — keeping the driven lightness and chroma ([`crate::drive::hold_hue`]).
/// `free`, or an `own` with no hue, leaves the driven colour as the model
/// sent it.
pub fn colour_to_signal(rgb: [f32; 3], own: Option<[f32; 3]>, hue_lock: &str) -> [f32; 3] {
    let held = if hue_lock == "hold" {
        own.map(|own| crate::drive::hold_hue(rgb, own)).unwrap_or(rgb)
    } else {
        rgb
    };
    let [hue, sat, level] = crate::venue::light_floats(held);
    [hue, sat, level]
}

/// The [`Signal`] a colour asks a fixture for (F4/F5): `level`/`hue`/`sat`
/// from [`colour_to_signal`], plus the white channels a non-RGB fixture reads
/// — `cct` from the colour's warmth (warm low, cool high) and `a`/`b` as the
/// warm/cool split of its level, so a warm/cool white or a temperature lamp
/// takes a colour too. What a profile does with those floats is its own
/// `[[channel]]`s through the one remap ([`Channel::raw`]), never code here.
pub fn colour_signal(rgb: [f32; 3], own: Option<[f32; 3]>, hue_lock: &str) -> Signal {
    let [hue, sat, level] = colour_to_signal(rgb, own, hue_lock);
    let warm = (rgb[0] - rgb[2]).clamp(-1.0, 1.0);
    let t = (1.0 - warm) * 0.5;
    Signal {
        level,
        hue,
        sat,
        strobe: 0.0,
        pan: 0.0,
        tilt: 0.0,
        a: level * (1.0 - t),
        b: level * t,
        cct: remap(warm, [-1.0, 1.0], [10000.0, 1800.0]),
        zoom: metal_schema::SIGNAL_FLOATS[9].default,
        focus: metal_schema::SIGNAL_FLOATS[10].default,
        iris: metal_schema::SIGNAL_FLOATS[11].default,
        frost: metal_schema::SIGNAL_FLOATS[12].default,
    }
}

/// The sRGB `0..1` a profile is showing in one packet — its `hue`/`sat`/
/// `level` channels read straight off the bytes that were on the wire, each
/// run back through the profile's own ranges ([`Channel::raw`]'s inverse,
/// [`remap`]) and [`crate::venue::rgb_from_light`]. This is how a fixture
/// node's live swatch is read (F10/F11/F14): the **last packet**, never a
/// recomputation of the colour it was built from. A profile with no hue/sat
/// channels answers grey at its level, which is what a white or temperature
/// lamp actually shows. `address` is the fixture's 1-based start (its row's
/// override or the profile's own), the same base [`Fixture::frame`] packs
/// against.
pub fn swatch_of(profile: &Fixture, address: u16, packet: &[u8; crate::wire::DMX_SLOTS]) -> [f32; 3] {
    let read = |selector: Selector, default: f32| -> f32 {
        let Some(ch) = profile.channels.iter().find(|c| c.signal == selector) else {
            return default;
        };
        let Some(coarse) = ch.coarse else { return default };
        let slot = |rele: u16| (address + rele).saturating_sub(1) as usize;
        let value = match ch.fine {
            Some(fine) => (((packet.get(slot(coarse)).copied().unwrap_or(0) as u32) << 8)
                | packet.get(slot(fine)).copied().unwrap_or(0) as u32) as f32,
            None => packet.get(slot(coarse)).copied().unwrap_or(0) as f32,
        };
        remap(value, ch.physical_range(), ch.signal_range())
    };
    crate::venue::rgb_from_light(
        read(Selector::Hue, 0.0),
        read(Selector::Sat, 0.0),
        read(Selector::Level, 0.0),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(super) const MOVING_HEAD: &str = r#"
label = "Moving Head"
universe = 2
address = 11

[[channel]]
signal = "pan"
coarse = 0
fine = 1
range = [-0.5, 0.5]

[[channel]]
signal = "tilt"
coarse = 2
fine = 3

[[channel]]
signal = "level"
coarse = 4

[[channel]]
signal = "hue"
coarse = 5

[[channel]]
signal = "sat"
coarse = 6

[[channel]]
signal = "strobe"
coarse = 7
"#;

    fn signal() -> Signal {
        Signal {
            level: 1.0,
            hue: 0.5,
            sat: 1.0,
            strobe: 12.0,
            pan: 0.0,
            tilt: 0.0,
            a: 0.0,
            b: 0.0,
            cct: 5600.0,
            zoom: 0.0,
            focus: 0.0,
            iris: 0.0,
            frost: 0.0,
        }
    }

    #[test]
    fn a_profile_reads_its_channels_and_defaults() {
        let f = parse("moving-head", MOVING_HEAD).unwrap();
        assert_eq!(f.model, "moving-head");
        assert_eq!(f.label, "Moving Head");
        assert_eq!(f.universe, 2);
        assert_eq!(f.address, 11);
        assert_eq!(f.channels.len(), 6);
        assert!(f.channels[0].fine.is_some());
        assert!(f.channels[2].fine.is_none());
    }

    /// F6: a profile reports its declared selectors in canonical page order,
    /// so a pan/tilt head offers pan and tilt while a plain wash does not.
    #[test]
    fn a_profile_reports_the_selectors_a_page_lays_out() {
        let head = parse("moving-head", MOVING_HEAD).unwrap();
        let names: Vec<&str> = head.selectors().iter().map(|s| s.name()).collect();
        assert_eq!(names, ["level", "hue", "sat", "strobe", "pan", "tilt"]);
        const WASH: &str = r#"
[[channel]]
signal = "level"
coarse = 0

[[channel]]
signal = "hue"
coarse = 1

[[channel]]
signal = "sat"
coarse = 2
"#;
        let wash = parse("wash", WASH).unwrap();
        let names: Vec<&str> = wash.selectors().iter().map(|s| s.name()).collect();
        assert_eq!(names, ["level", "hue", "sat"]);
        assert!(!names.contains(&"pan") && !names.contains(&"tilt"), "a wash has no pan/tilt");
    }

    /// M6: a profile's safety envelope clamps the state before the wire — the
    /// dimmer to `max_dimmer`, the strobe to zero when the profile refuses
    /// it, and pan/tilt out of a no-go zone. A profile that declares none
    /// leaves the signal alone.
    #[test]
    fn a_profile_clamps_a_heads_dimmer_and_pan_before_the_wire() {
        const SAFE: &str = r#"
[[channel]]
signal = "level"
coarse = 0

[[channel]]
signal = "pan"
coarse = 1

[[channel]]
signal = "tilt"
coarse = 2

[[channel]]
signal = "strobe"
coarse = 3

[limits]
max_dimmer = 0.5
no_strobe = true

[[limits.no_go]]
pan = [-10.0, 10.0]
tilt = [-30.0, 30.0]
"#;
        let f = parse("safe-head", SAFE).unwrap();
        assert_eq!(f.limits.max_dimmer, 0.5);
        assert!(f.limits.no_strobe);
        let mut s = signal();
        f.limits.clamp_signal(&mut s);
        assert_eq!(s.level, 0.5, "the dimmer is capped");
        assert_eq!(s.strobe, 0.0, "the profile refuses strobe");
        let (pan, tilt) = f.limits.clamp_pose(2.0, 5.0);
        assert!(pan <= -10.0 || pan >= 10.0, "pushed out of the no-go zone: {pan}");
        assert!(tilt <= -30.0 || tilt >= 30.0 || pan.abs() >= 10.0, "out of the zone: {tilt}");
        // A profile with no `[limits]` does not touch the signal.
        let plain = parse("moving-head", MOVING_HEAD).unwrap();
        let mut s = signal();
        plain.limits.clamp_signal(&mut s);
        assert_eq!(s.level, 1.0);
        assert_eq!(s.strobe, 12.0);
    }

    #[test]
    fn a_selector_round_trips_through_its_names() {
        for selector in [
            Selector::Level,
            Selector::Hue,
            Selector::Sat,
            Selector::Strobe,
            Selector::Pan,
            Selector::Tilt,
            Selector::A,
            Selector::B,
            Selector::Cct,
            Selector::Zoom,
            Selector::Focus,
            Selector::Iris,
            Selector::Frost,
        ] {
            assert_eq!(Selector::parse(selector.name()), Some(selector));
        }
        assert_eq!(Selector::parse("sig_pan"), Some(Selector::Pan));
        assert_eq!(Selector::parse("gobo"), None);
    }

    #[test]
    fn remap_is_linear_and_clamped() {
        assert_eq!(remap(0.0, [0.0, 1.0], [0.0, 255.0]), 0.0);
        assert_eq!(remap(1.0, [0.0, 1.0], [0.0, 255.0]), 255.0);
        assert_eq!(remap(0.5, [0.0, 1.0], [0.0, 255.0]), 127.5);
        // Clamped, not wrapped: beyond the declared range stays at the ends.
        assert_eq!(remap(2.0, [0.0, 1.0], [0.0, 255.0]), 255.0);
        assert_eq!(remap(-1.0, [0.0, 1.0], [0.0, 255.0]), 0.0);
        // A centred pan of 0 on a ±0.5 window is mid-travel.
        assert_eq!(remap(0.0, [-0.5, 0.5], [0.0, 65535.0]), 32767.5);
    }

    #[test]
    fn a_16_bit_channel_splits_into_coarse_and_fine() {
        let f = parse("moving-head", MOVING_HEAD).unwrap();
        let pan = &f.channels[0];
        let bytes = pan.bytes(&signal());
        // pan 0.0 on [-0.5, 0.5] -> half of 65535 -> rounds to 32768 = 0x8000.
        assert_eq!(bytes, vec![(0, 0x80), (1, 0x00)]);
    }

    #[test]
    fn an_8_bit_channel_stays_one_byte() {
        let f = parse("moving-head", MOVING_HEAD).unwrap();
        let level = &f.channels[2];
        assert_eq!(level.bytes(&signal()), vec![(4, 255)]);
    }

    #[test]
    fn a_head_overrides_the_profile_address_and_packs_a_frame() {
        let f = parse("moving-head", MOVING_HEAD).unwrap();
        let head = Head {
            fixture: "moving-head".into(),
            universe: None,
            address: Some(101),
        };
        assert_eq!(f.universe_of(&head), 2);
        let frame = f.frame(&head, &signal());
        // 8-bit level at rel 4 -> abs 105; the pan pair at rel 0/1 -> 101/102.
        assert!(frame.contains(&(105, 255)));
        assert!(frame.contains(&(101, 0x80)));
        assert!(frame.contains(&(102, 0x00)));
        assert_eq!(frame.len(), 8, "6 channels, pan and tilt are 16-bit");
        // Sorted by address.
        let addresses: Vec<u16> = frame.iter().map(|(a, _)| *a).collect();
        let mut sorted = addresses.clone();
        sorted.sort_unstable();
        assert_eq!(addresses, sorted);
    }

    #[test]
    fn a_profile_that_repeats_a_signal_or_a_slot_is_refused() {
        let twice_signal = r#"
[[channel]]
signal = "pan"
coarse = 0
[[channel]]
signal = "pan"
coarse = 1
"#;
        assert!(parse("bad", twice_signal).is_err());

        let twice_slot = r#"
[[channel]]
signal = "pan"
coarse = 0
[[channel]]
signal = "tilt"
coarse = 0
"#;
        assert!(parse("bad", twice_slot).is_err());

        let empty = r#"label = "nothing""#;
        assert!(parse("bad", empty).is_err());
    }

    #[test]
    fn a_missing_directory_loads_as_empty() {
        let root = std::env::temp_dir().join("fw-fixture-none-such");
        assert!(load(&root).is_empty());
    }

    #[test]
    fn the_fallback_is_deterministic_and_bounded() {
        let signature = [0.25, 0.5, 0.75, 0.1, -2.0, 2.0, 0.0, 0.9];
        let a = fallback_signals(&signature, 4);
        let b = fallback_signals(&signature, 4);
        assert_eq!(a, b);
        assert_eq!(a.len(), 4);
        for signal in &a {
            assert!((0.0..=1.0).contains(&signal.level));
            assert!((0.0..=1.0).contains(&signal.sat));
            assert!((0.0..=24.0).contains(&signal.strobe));
            assert!((-1.0..=1.0).contains(&signal.pan));
            assert!((-1.0..=1.0).contains(&signal.tilt));
        }
        assert!(fallback_signals(&signature, 0).is_empty());
        assert_eq!(fallback_signals(&[], 1).len(), 1);
    }

    /// The byte a profile writes for `signal` at `head`'s address, read back out
    /// of the slots [`Fixture::frame`] produced.
    fn byte_for(p: &Fixture, head: &Head, signal: Selector, slots: &[(u16, u8)]) -> Option<u8> {
        let c = p.channels.iter().find(|c| c.signal == signal)?;
        let addr = p.address_of(head) + c.coarse?;
        slots.iter().find(|(a, _)| *a == addr).map(|(_, b)| *b)
    }

    /// F5: a colour reaches **every** profile in `data/fixtures/` through the
    /// one remap — the profile's own `[[channel]]`s decide which floats it
    /// reads, so a `cct` or a warm/cool `a`/`b` lamp takes a colour too, with
    /// no per-fixture code. The list is walked from the directory (rule 7),
    /// never hand-written.
    #[test]
    fn a_colour_reaches_every_profile_through_its_own_channels() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data");
        let profiles = load(&root);
        assert!(!profiles.is_empty(), "data/fixtures carries profiles");
        let red = colour_signal([1.0, 0.0, 0.0], None, "free");
        let blue = colour_signal([0.0, 0.0, 1.0], None, "free");
        let mut cct_profiles = 0;
        let mut white_profiles = 0;
        for p in &profiles {
            let head = Head { fixture: p.model.clone(), universe: None, address: None };
            if p.wire == WireKind::Bluetooth {
                // No slots: `light_state` is the path, and a bright colour is
                // not dark.
                assert!(p.light_state(&red).level > 0.0, "{}: a bright colour is dark", p.model);
                if p.channels.iter().any(|c| c.signal == Selector::Cct) {
                    cct_profiles += 1;
                    let warm = p.light_state(&red).cct_kelvin;
                    let cool = p.light_state(&blue).cct_kelvin;
                    match (warm, cool) {
                        (Some(w), Some(c)) => assert!(w < c, "{}: red must read warmer than blue ({w} !< {c})", p.model),
                        _ => panic!("{}: a cct channel must reach `light_state`", p.model),
                    }
                }
                if p.channels.iter().any(|c| c.signal == Selector::Hue) {
                    assert!(p.light_state(&red).hsi.is_some(), "{}: a saturated colour asks for HSI", p.model);
                }
                continue;
            }
            let slots = p.frame(&head, &red);
            if let (Some(level), Some(byte)) = (
                p.channels.iter().find(|c| c.signal == Selector::Level),
                byte_for(p, &head, Selector::Level, &slots),
            ) {
                assert_eq!(byte, level.raw(&red).round().clamp(0.0, 255.0) as u8, "{}: level", p.model);
            }
            if let (Some(hue), Some(byte)) = (
                p.channels.iter().find(|c| c.signal == Selector::Hue),
                byte_for(p, &head, Selector::Hue, &slots),
            ) {
                assert_eq!(byte, hue.raw(&red).round().clamp(0.0, 255.0) as u8, "{}: hue", p.model);
            }
            if p.channels.iter().any(|c| c.signal == Selector::Cct) {
                cct_profiles += 1;
                let (Some(w), Some(c)) = (
                    byte_for(p, &head, Selector::Cct, &p.frame(&head, &red)),
                    byte_for(p, &head, Selector::Cct, &p.frame(&head, &blue)),
                ) else {
                    panic!("{}: a cct channel must be written", p.model)
                };
                assert!(w < c, "{}: red must read warmer than blue ({w} !< {c})", p.model);
            }
            if p.channels.iter().any(|c| c.signal == Selector::A)
                && p.channels.iter().any(|c| c.signal == Selector::B)
            {
                white_profiles += 1;
                let (Some(a), Some(b)) = (
                    byte_for(p, &head, Selector::A, &slots),
                    byte_for(p, &head, Selector::B, &slots),
                ) else {
                    panic!("{}: a warm/cool pair must be written", p.model)
                };
                assert!(a > b, "{}: red drives the warm channel over the cool ({a} !> {b})", p.model);
                let cool = p.frame(&head, &blue);
                let (ca, cb) = (
                    byte_for(p, &head, Selector::A, &cool).unwrap(),
                    byte_for(p, &head, Selector::B, &cool).unwrap(),
                );
                assert!(cb > ca, "{}: blue drives the cool channel over the warm", p.model);
            }
        }
        assert!(cct_profiles > 0, "the directory must carry a temperature profile");
        assert!(white_profiles > 0, "the directory must carry a warm/cool profile");
    }

    /// F4/F5: `colour_to_signal` is `light_floats` with `hue_lock` applied;
    /// `colour_signal` adds the white channels a non-RGB profile reads.
    #[test]
    fn colour_to_signal_is_light_floats_with_the_hue_lock() {
        let rgb = [0.2, 0.6, 0.9];
        assert_eq!(colour_to_signal(rgb, None, "free"), crate::venue::light_floats(rgb));
        let own = [0.9, 0.2, 0.2];
        // `hold` is the held colour's floats — the driven hue swapped for the
        // model's own.
        let held = crate::drive::hold_hue(rgb, own);
        assert_eq!(colour_to_signal(rgb, Some(own), "hold"), crate::venue::light_floats(held));
        // A grey has no hue to hold: the driven colour is untouched.
        assert_eq!(colour_to_signal(rgb, Some([1.0, 1.0, 1.0]), "hold"), crate::venue::light_floats(rgb));

        let warm = colour_signal([1.0, 0.0, 0.0], None, "free");
        let cool = colour_signal([0.0, 0.0, 1.0], None, "free");
        assert!(warm.cct < cool.cct, "warm below cool: {} vs {}", warm.cct, cool.cct);
        assert!(warm.a > warm.b, "red drives the warm white channel");
        assert!(cool.b > cool.a, "blue drives the cool white channel");
    }

    /// F10/F11/F14: a swatch read back off the packet is the colour the bytes
    /// on the wire stand for — no re-run of the model, just the profile's
    /// channels decoded.
    #[test]
    fn a_packet_decodes_back_to_the_swatch_it_carries() {
        let head = parse("moving-head", MOVING_HEAD).unwrap();
        let mut packet = [0u8; crate::wire::DMX_SLOTS];
        packet[14] = 255; // level (address 11 + coarse 4 - 1)
        packet[15] = 0; // hue 0 = red
        packet[16] = 255; // sat
        let red = swatch_of(&head, 11, &packet);
        assert!((red[0] - 1.0).abs() < 1e-3 && red[1] < 1e-3 && red[2] < 1e-3, "{red:?}");

        // A profile with only a dimmer answers grey at its level.
        const DIMMER: &str = r#"
[[channel]]
signal = "level"
coarse = 0
"#;
        let dim = parse("dimmer", DIMMER).unwrap();
        let mut dark = [0u8; crate::wire::DMX_SLOTS];
        dark[0] = 128;
        let grey = swatch_of(&dim, 1, &dark);
        assert!(grey.iter().all(|v| (v - 128.0 / 255.0).abs() < 1e-3), "{grey:?}");
    }
}

#[cfg(test)]
mod force_tests {
    use super::*;

    #[test]
    fn empty_config_round_trips_to_nothing_forced() {
        let node = metal_graph::Node::new("f1", "out.sacn", [0.0, 0.0]);
        let cfg = FixtureConfig::from_node(&node);
        assert!(cfg.is_empty());
        assert_eq!(cfg.forced, None);
    }

    #[test]
    fn forced_black_round_trips_through_the_node_config() {
        let mut node = metal_graph::Node::new("f1", "out.sacn", [0.0, 0.0]);
        node.config = FixtureConfig { forced: Some([0.0, 0.0, 0.0]), patch: crate::wire::PatchMode::Colour }.to_config();
        let back = FixtureConfig::from_node(&node);
        assert_eq!(back.forced, Some([0.0, 0.0, 0.0]));
    }

    #[test]
    fn clearing_forced_writes_the_config_back_to_empty() {
        let mut node = metal_graph::Node::new("f1", "out.sacn", [0.0, 0.0]);
        node.config = FixtureConfig { forced: Some([0.0, 0.0, 0.0]), patch: crate::wire::PatchMode::Colour }.to_config();
        node.config = FixtureConfig::default().to_config();
        assert!(FixtureConfig::from_node(&node).is_empty());
    }

    /// Blackout's write keeps the wire: `forced` goes on and comes off, and
    /// the node's own settings (universe, unicast, a frame-driven node's
    /// heads) are exactly what they were before.
    #[test]
    fn set_forced_keeps_every_other_key_of_the_node() {
        let mut node = metal_graph::Node::new("f1", "out.sacn", [0.0, 0.0]);
        let before = r#"{"heads":[{"fixture":"example-wash","sample":[0.5,0.5]}],"universe":7}"#;
        node.config = metal_graph::Config::from_json_str(before).unwrap();

        set_forced(&mut node, Some([0.0, 0.0, 0.0]));
        assert_eq!(FixtureConfig::from_node(&node).forced, Some([0.0, 0.0, 0.0]));
        assert_eq!(node.config.get("universe"), Some(&serde_json::json!(7)));
        assert!(node.config.get("heads").is_some(), "{}", node.config.to_json_str());

        set_forced(&mut node, None);
        assert_eq!(node.config.to_json_str(), before, "released: byte for byte what it was");

        let mut bare = metal_graph::Node::new("f2", "out.sacn", [0.0, 0.0]);
        set_forced(&mut bare, Some([0.0, 0.0, 0.0]));
        set_forced(&mut bare, None);
        assert!(bare.config.is_empty(), "a node with no config goes back to none");
    }

    fn wire() -> Signal {
        Signal {
            level: 0.8,
            hue: 0.3,
            sat: 0.9,
            strobe: 12.0,
            pan: 0.25,
            tilt: -0.5,
            a: 0.1,
            b: 0.2,
            cct: 5600.0,
            zoom: 0.2,
            focus: 0.3,
            iris: 0.4,
            frost: 0.5,
        }
    }

    /// `S.O.1`: the fixture half of blackout, at the signal. Forced black
    /// zeroes what a colour is and the strobe, and leaves the beam where it
    /// was; nothing forced is the wire, bit for bit.
    #[test]
    fn forced_wins_over_the_wire_and_leaves_the_beam_where_it_was() {
        let cfg = FixtureConfig { forced: Some([0.0, 0.0, 0.0]), patch: crate::wire::PatchMode::Colour };
        let out = cfg.drive(&wire());
        assert_eq!((out.level, out.hue, out.sat, out.strobe), (0.0, 0.0, 0.0, 0.0));
        assert_eq!((out.pan, out.tilt, out.a, out.b), (0.25, -0.5, 0.1, 0.2));

        let red = FixtureConfig { forced: Some([1.0, 0.0, 0.0]), patch: crate::wire::PatchMode::Colour }.drive(&wire());
        assert_eq!((red.level, red.hue, red.sat), (1.0, 0.0, 1.0));
        let green = FixtureConfig { forced: Some([0.0, 1.0, 0.0]), patch: crate::wire::PatchMode::Colour }.drive(&wire());
        assert!((green.hue - 1.0 / 3.0).abs() < 1e-6);

        assert_eq!(FixtureConfig::default().drive(&wire()), wire());
    }

    /// The whole path a wire backend will take, against the same profile
    /// `data/fixtures/` ships: a blacked-out graph packs a zero level byte
    /// where the wire asked for 204, and releasing it packs the wire again.
    #[test]
    fn drive_head_reads_forced_off_the_graph_before_the_wire() {
        let root = std::env::temp_dir().join(format!(
            "fw-fixture-drive-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        metal_vfs::create_dir_all(dir(&root)).unwrap();
        metal_vfs::write(dir(&root).join("probe.toml"), super::tests::MOVING_HEAD).unwrap();
        let head = Head { fixture: "probe".into(), universe: None, address: Some(1) };

        let mut g = metal_graph::Graph::new("device-probe", "t", metal_graph::Domain::Live);
        g.add_node("fixture", "out.sacn", [0.0, 0.0]);
        // Level is the profile's channel at relative slot 4: absolute 5.
        let live = drive_head(&root, &head, Some(&g), None, &wire()).expect("profile found");
        assert!(live.contains(&(5, 204)), "wire level 0.8 → 204: {live:?}");

        // A timed overlay's cover reaches the head too: solid red is full
        // level, hue 0, full sat — and the graph's own forced black, when
        // both are on, still wins.
        let red = drive_head(&root, &head, Some(&g), Some([255, 0, 0]), &wire()).unwrap();
        assert!(red.contains(&(5, 255)) && red.contains(&(6, 0)) && red.contains(&(7, 255)), "{red:?}");

        let node = g.nodes.iter_mut().find(|n| n.kind == "out.sacn").unwrap();
        node.config = FixtureConfig { forced: Some([0.0, 0.0, 0.0]), patch: crate::wire::PatchMode::Colour }.to_config();
        assert_eq!(forced_on(&g), Some([0.0, 0.0, 0.0]));
        let black = drive_head(&root, &head, Some(&g), Some([255, 0, 0]), &wire()).expect("profile found");
        assert!(black.contains(&(5, 0)), "forced black → level 0, over the cover: {black:?}");
        assert!(black.contains(&(8, 0)), "and no strobe: {black:?}");
        // The pan pair is the wire's, untouched by blackout.
        let pan = |f: &[(u16, u8)]| (f[0], f[1]);
        assert_eq!(pan(&black), pan(&live), "the beam stays where it was");

        assert_eq!(
            drive_head(&root, &Head { fixture: "nope".into(), universe: None, address: None }, None, None, &wire()),
            None,
            "a missing profile is a skip"
        );
        metal_vfs::remove_dir_all(&root).ok();
    }

    /// F3: the `signal` colour source of a `venue.fixture` node packs through
    /// `drive_head`, so the graph's blackout still zeroes `level`/`hue`/`sat`
    /// (and the strobe) and leaves `pan`/`tilt` exactly where the wire put
    /// them. The node is a pointer: the row names the profile, the node copies
    /// nothing.
    #[test]
    fn a_venue_fixture_signal_source_keeps_blackout_and_leaves_the_beam() {
        let root = std::env::temp_dir().join(format!(
            "fw-fixture-venue-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = metal_vfs::remove_dir_all(&root);
        metal_vfs::create_dir_all(dir(&root)).unwrap();
        metal_vfs::write(dir(&root).join("probe.toml"), super::tests::MOVING_HEAD).unwrap();

        let venue = crate::venue::Venue::parse(
            r#"
            [venue]
            id = "scratch"
            name = "scratch"
            setting = "indoor"

            [[fixture]]
            id = "head-1"
            name = "head 1"
            kind = "wash"
            profile = "probe"
            wire = "sacn"
        "#,
        )
        .expect("the row parses");

        let graph = metal_graph::Graph::new("device-probe", "t", metal_graph::Domain::Live);
        let live = drive_venue_fixture_signal(&root, &venue, "head-1", Some(&graph), None, &wire())
            .expect("the row's profile is found");
        // Universe 2 address 11: level coarse 4 → 15, hue 16, sat 17, strobe 18,
        // pan 11/12, tilt 13/14.
        assert!(live.contains(&(15, 204)), "wire level 0.8 → 204: {live:?}");

        let mut g = graph;
        let out = g.add_node("out", "out.sacn", [0.0, 0.0]);
        g.node_mut(&out).unwrap().config = FixtureConfig { forced: Some([0.0, 0.0, 0.0]), patch: crate::wire::PatchMode::Colour }.to_config();
        let black = drive_venue_fixture_signal(&root, &venue, "head-1", Some(&g), None, &wire())
            .expect("the row's profile is found");
        assert!(black.contains(&(15, 0)), "forced black → level 0: {black:?}");
        assert!(black.contains(&(18, 0)), "and no strobe: {black:?}");
        let bytes = |f: &[(u16, u8)], at: u16| f.iter().filter(|(a, _)| *a == at).map(|(_, b)| *b).collect::<Vec<_>>();
        assert_eq!(bytes(&black, 11), bytes(&live, 11), "pan coarse stays");
        assert_eq!(bytes(&black, 12), bytes(&live, 12), "pan fine stays");
        assert_eq!(bytes(&black, 13), bytes(&live, 13), "tilt coarse stays");
        assert_eq!(bytes(&black, 14), bytes(&live, 14), "tilt fine stays");

        assert!(
            drive_venue_fixture_signal(&root, &venue, "gone", Some(&g), None, &wire()).is_none(),
            "a row that is gone is a skip"
        );
        metal_vfs::remove_dir_all(&root).ok();
    }
}
