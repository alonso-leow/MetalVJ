//! The MIDI wire, decoded — what a class-compliant controller (a Novation
//! FLkey Mini, a Launchpad, any USB-MIDI box) sends, in the shape the control
//! surface binds against.
//!
//! Three pure pieces and one optional one:
//! - [`decode`]: one channel message → [`Msg`] (note on/off, CC, pitch bend).
//!   A Note On with velocity 0 is the running-status Note Off (MIDI 1.0).
//! - [`Msg::input`]: the channel-free id a binding is keyed by (`note:60`,
//!   `cc:21`, `bend`) — the same physical control on any channel binds once.
//! - [`Dial`]: turns a knob's value into steps, for an absolute pot (0..127,
//!   the FLkey's knobs) or a relative encoder (two's complement or binary
//!   offset), so a "search dial" or "speed dial" moves one notch per detent.
//! - [`devices`]: known controllers' faces — every key, pad, knob and button
//!   where it sits and what it sends — for the grid to take the shape of.
//! - [`ports`] (feature `ports`): the `midir` connection — list the inputs and
//!   open every one whose name matches, each message handed to a callback.

/// One decoded channel message. `channel` is 0..=15.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Msg {
    NoteOn { channel: u8, note: u8, velocity: u8 },
    NoteOff { channel: u8, note: u8 },
    Cc { channel: u8, cc: u8, value: u8 },
    /// 14-bit, 0..=16383, centre 8192.
    PitchBend { channel: u8, value: u16 },
}

/// Decodes one message. Anything that is not a note, a CC or a pitch bend
/// (system, clock, aftertouch, program change) is `None`, and so is a message
/// too short for its status.
pub fn decode(bytes: &[u8]) -> Option<Msg> {
    let (&status, data) = bytes.split_first()?;
    let channel = status & 0x0F;
    let d = |i: usize| data.get(i).copied().map(|b| b & 0x7F);
    match status & 0xF0 {
        0x80 => Some(Msg::NoteOff { channel, note: d(0)? }),
        0x90 => {
            let (note, velocity) = (d(0)?, d(1)?);
            Some(if velocity == 0 {
                Msg::NoteOff { channel, note }
            } else {
                Msg::NoteOn { channel, note, velocity }
            })
        }
        0xB0 => Some(Msg::Cc { channel, cc: d(0)?, value: d(1)? }),
        0xE0 => Some(Msg::PitchBend { channel, value: (d(0)? as u16) | ((d(1)? as u16) << 7) }),
        _ => None,
    }
}

/// What a message means to a binding: which control, and what it did.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Edge {
    /// A key or pad went down; `velocity` in 0..=1.
    Press { velocity: f32 },
    /// A key or pad came up.
    Release,
    /// A knob, fader or bend moved to `value`, 0..=1 (raw byte kept for
    /// [`Dial`]).
    Value { value: f32, raw: u8 },
}

impl Msg {
    /// The binding id of the control this message came from, channel-free:
    /// `note:<n>`, `cc:<n>`, `bend`.
    pub fn input(&self) -> String {
        match self {
            Msg::NoteOn { note, .. } | Msg::NoteOff { note, .. } => note_input(*note),
            Msg::Cc { cc, .. } => format!("cc:{cc}"),
            Msg::PitchBend { .. } => "bend".to_string(),
        }
    }

    /// 0..=15.
    pub fn channel(&self) -> u8 {
        match *self {
            Msg::NoteOn { channel, .. }
            | Msg::NoteOff { channel, .. }
            | Msg::Cc { channel, .. }
            | Msg::PitchBend { channel, .. } => channel,
        }
    }

    /// The input id with its channel (1-based): `note:36@10` — what tells a
    /// pad on channel 10 from the key of the same note on channel 1.
    pub fn input_on_channel(&self) -> String {
        format!("{}@{}", self.input(), self.channel() + 1)
    }

    /// What the control did. A CC is always a value: a pad in CC mode sends
    /// 127/0, and the surface reads `> 0.5` as down where it wants a button.
    pub fn edge(&self) -> Edge {
        match *self {
            Msg::NoteOn { velocity, .. } => Edge::Press { velocity: velocity as f32 / 127.0 },
            Msg::NoteOff { .. } => Edge::Release,
            Msg::Cc { value, .. } => Edge::Value { value: value as f32 / 127.0, raw: value },
            Msg::PitchBend { value, .. } => Edge::Value {
                value: value as f32 / 16383.0,
                raw: (value >> 7) as u8,
            },
        }
    }
}

/// The input id of note `n` — what an on-screen piano key and a hardware key
/// share, so a key bound on one fires from the other.
pub fn note_input(n: u8) -> String {
    format!("note:{n}")
}

/// The note number an input id names, if it is a note (with or without its
/// channel).
pub fn input_note(input: &str) -> Option<u8> {
    plain_input(input).strip_prefix("note:")?.parse().ok()
}

/// An input id without its channel: `note:36@10` → `note:36`.
pub fn plain_input(input: &str) -> &str {
    input.split_once('@').map_or(input, |(p, _)| p)
}

/// Whether a bound input and a heard one are the same control: equal, or
/// one of them names no channel and the rest agrees (`cc:21` answers
/// `cc:21@1`; `note:36@10` does not answer `note:36@1`).
pub fn same_control(bound: &str, heard: &str) -> bool {
    bound == heard
        || ((!bound.contains('@') || !heard.contains('@')) && plain_input(bound) == plain_input(heard))
}

/// `C4`-style name of a note (MIDI 60 is `C4`).
pub fn note_name(n: u8) -> String {
    const NAMES: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
    format!("{}{}", NAMES[(n % 12) as usize], (n as i32 / 12) - 1)
}

/// Whether note `n` is a black key.
pub fn is_black(n: u8) -> bool {
    matches!(n % 12, 1 | 3 | 6 | 8 | 10)
}

/// How a knob reports itself.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DialMode {
    /// A pot: the value is the position, 0..=127. Steps come from the change.
    #[default]
    Absolute,
    /// An endless encoder, two's complement: 1..=63 up, 65..=127 down.
    Relative,
    /// An endless encoder, binary offset: 64 is still, above up, below down.
    Offset,
}

/// Turns a knob's raw values into whole steps. An absolute pot is divided into
/// `notch`-wide detents so a slow turn moves one entry at a time; an encoder
/// already sends steps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Dial {
    pub mode: DialMode,
    /// Raw units per step for an absolute pot (`4` → 32 steps per sweep).
    pub notch: u8,
    last: Option<u8>,
    acc: i16,
}

impl Default for Dial {
    fn default() -> Self {
        Dial::new(DialMode::Absolute)
    }
}

impl Dial {
    pub fn new(mode: DialMode) -> Dial {
        Dial { mode, notch: 4, last: None, acc: 0 }
    }

    /// The steps this raw value makes (positive clockwise). The first value an
    /// absolute pot sends only fixes its position and steps nothing.
    pub fn step(&mut self, raw: u8) -> i32 {
        match self.mode {
            DialMode::Relative => match raw {
                0 => 0,
                1..=63 => raw as i32,
                _ => raw as i32 - 128,
            },
            DialMode::Offset => raw as i32 - 64,
            DialMode::Absolute => {
                let Some(last) = self.last.replace(raw) else {
                    return 0;
                };
                // Ends always step: a pot slammed to 0 or 127 lands there.
                self.acc += raw as i16 - last as i16;
                let notch = self.notch.max(1) as i16;
                let steps = self.acc / notch;
                self.acc -= steps * notch;
                if steps == 0 && raw != last && (raw == 0 || raw == 127) {
                    self.acc = 0;
                    return if raw == 0 { -1 } else { 1 };
                }
                steps as i32
            }
        }
    }
}

/// Known controllers, by port name: what the surface can say about a box
/// without asking the operator. Every one of them is class-compliant; the
/// profile only names it and its default knob behaviour.
pub fn profile(port_name: &str) -> Option<&'static str> {
    let lower = port_name.to_lowercase();
    if lower.contains("flkey mini") {
        Some("Novation FLkey Mini")
    } else if lower.contains("flkey") {
        Some("Novation FLkey")
    } else if lower.contains("launchkey") {
        Some("Novation Launchkey")
    } else if lower.contains("launchpad") {
        Some("Novation Launchpad")
    } else if lower.contains("apc") {
        Some("Akai APC")
    } else if lower.contains("mpk") {
        Some("Akai MPK")
    } else if lower.contains("nanokontrol") {
        Some("Korg nanoKONTROL")
    } else {
        None
    }
}

pub mod devices;

#[cfg(feature = "ports")]
pub mod ports;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notes_decode_and_velocity_zero_is_a_note_off() {
        assert_eq!(decode(&[0x90, 60, 100]), Some(Msg::NoteOn { channel: 0, note: 60, velocity: 100 }));
        assert_eq!(decode(&[0x9F, 60, 0]), Some(Msg::NoteOff { channel: 15, note: 60 }));
        assert_eq!(decode(&[0x80, 61, 64]), Some(Msg::NoteOff { channel: 0, note: 61 }));
    }

    #[test]
    fn cc_and_bend_decode_and_the_rest_is_ignored() {
        assert_eq!(decode(&[0xB1, 21, 127]), Some(Msg::Cc { channel: 1, cc: 21, value: 127 }));
        assert_eq!(decode(&[0xE0, 0, 64]), Some(Msg::PitchBend { channel: 0, value: 8192 }));
        assert_eq!(decode(&[0xF8]), None);
        assert_eq!(decode(&[0xC0, 5]), None);
        assert_eq!(decode(&[0x90, 60]), None, "a truncated note is not a note");
        assert_eq!(decode(&[]), None);
    }

    #[test]
    fn input_ids_are_channel_free() {
        let a = decode(&[0x90, 60, 1]).unwrap();
        let b = decode(&[0x83, 60, 0]).unwrap();
        assert_eq!(a.input(), "note:60");
        assert_eq!(a.input(), b.input());
        assert_eq!(decode(&[0xB5, 7, 0]).unwrap().input(), "cc:7");
        assert_eq!(input_note("note:60"), Some(60));
        assert_eq!(input_note("cc:60"), None);
        assert_eq!(a.input_on_channel(), "note:60@1");
        assert_eq!(b.input_on_channel(), "note:60@4");
        assert_eq!(input_note("note:36@10"), Some(36));
    }

    #[test]
    fn a_channel_tells_a_pad_from_a_key_but_a_bare_id_answers_any() {
        assert!(same_control("note:36@10", "note:36@10"));
        assert!(!same_control("note:36@10", "note:36@1"));
        assert!(same_control("cc:21", "cc:21@1"));
        assert!(same_control("note:60@1", "note:60"), "the on-screen piano is channel-free");
        assert!(!same_control("cc:21", "cc:22@1"));
        assert_eq!(plain_input("bend@1"), "bend");
    }

    #[test]
    fn edges_name_press_release_and_value() {
        assert_eq!(decode(&[0x90, 60, 127]).unwrap().edge(), Edge::Press { velocity: 1.0 });
        assert_eq!(decode(&[0x90, 60, 0]).unwrap().edge(), Edge::Release);
        assert_eq!(decode(&[0xB0, 1, 0]).unwrap().edge(), Edge::Value { value: 0.0, raw: 0 });
    }

    #[test]
    fn note_names_and_black_keys() {
        assert_eq!(note_name(60), "C4");
        assert_eq!(note_name(61), "C#4");
        assert_eq!(note_name(48), "C3");
        assert!(is_black(61) && !is_black(60) && !is_black(64) && is_black(70));
    }

    #[test]
    fn an_absolute_pot_steps_once_per_notch_and_its_first_value_only_positions_it() {
        let mut d = Dial::new(DialMode::Absolute);
        assert_eq!(d.step(64), 0);
        assert_eq!(d.step(66), 0);
        assert_eq!(d.step(68), 1);
        assert_eq!(d.step(60), -2);
        assert_eq!(d.step(127), 16);
        let mut d = Dial::new(DialMode::Absolute);
        d.step(1);
        assert_eq!(d.step(0), -1, "a pot slammed to an end always steps");
    }

    #[test]
    fn encoders_send_their_own_steps() {
        let mut r = Dial::new(DialMode::Relative);
        assert_eq!(r.step(1), 1);
        assert_eq!(r.step(127), -1);
        assert_eq!(r.step(3), 3);
        let mut o = Dial::new(DialMode::Offset);
        assert_eq!(o.step(65), 1);
        assert_eq!(o.step(62), -2);
    }

    #[test]
    fn known_boxes_are_named() {
        assert_eq!(profile("FLkey Mini:FLkey Mini MIDI 1 24:0"), Some("Novation FLkey Mini"));
        assert_eq!(profile("Midi Through:Midi Through Port-0 14:0"), None);
    }
}
