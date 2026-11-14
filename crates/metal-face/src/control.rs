//! The rig controls (23 for speech, then the appended expression muscles),
//! and a value of all of them at once.
//!
//! # One table, everything derived from it
//!
//! The controls are declared once, in the `controls!` invocation below, with
//! their name, range, rest value and spring. The enum, [`FaceControl::ALL`],
//! [`FaceControl::COUNT`], the names and the specs are all generated from
//! that one list, so there is no second place where a count or an order could
//! drift. Tests count from `ALL`, never from a literal (plan rule 7).
//!
//! # The names are a contract, and append-only
//!
//! [`FaceControl::name`] becomes the name of a float in the `monster` model's
//! schema, and through it a lane name, a binding target and a key in saved
//! sequences. So the list follows plan rule 2: **a new control is appended at
//! the end, and an existing one is never renamed, reordered or removed.** A
//! retired control stays in the table and simply stops being written. The
//! enum's discriminant is its index in `ALL`, and [`Controls`] stores values
//! by that index, which is a second reason the order is frozen.
//!
//! # Ranges
//!
//! A control is either one-sided (`0..1`, "how much") or signed (`−1..1`,
//! "which way and how much": `jaw_side`, `tongue_curl` and the three head
//! angles). The per-side expression controls (`*_l` / `*_r`) are one-sided
//! and add to their symmetric sibling. Every rest value is 0, which is what makes "rest controls give the
//! neutral topology" a property of the deformation code rather than of a
//! special case.
//!
//! # Springs
//!
//! Each control has a critically damped spring with two natural frequencies
//! (rad/s): **attack**, used while the target is further from rest than the
//! current value (the control is being pushed), and **release**, used while it
//! is coming back. A critically damped spring starting from rest reaches about
//! 95 % of a step in `4.74 / ω` seconds, which is how the numbers below were
//! chosen against the speech timings in `the voice design notes`:
//!
//! - `lip_close` attacks at 85 rad/s (≈ 56 ms to 95 %): a p/b/m closure lasts
//!   50–100 ms, and a closure that arrives late is a closure nobody sees.
//! - `jaw_open` attacks at 38 rad/s (≈ 125 ms): the jaw is the heaviest thing
//!   in the mouth and carries a syllable, not a consonant.
//! - `tongue_up` / `tongue_curl` are the fastest, 110 rad/s (≈ 43 ms): the
//!   tongue tip flicks for t/d/n/l faster than anything else in the face.
//! - The other lip shapes sit between the lips' closure and the jaw.
//! - `blink` attacks fast and releases slower, as real lids do.
//! - The head is slowest by far (6–8 rad/s): it nods and drifts, it does not
//!   articulate.

use std::fmt;
use std::ops::{Index, IndexMut};

use serde::de::{MapAccess, Visitor};
use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// The two natural frequencies of one control's spring, in rad/s.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Spring {
    /// Used while the target is further from rest than the current value.
    pub attack: f32,
    /// Used while the value is coming back toward rest.
    pub release: f32,
}

/// Everything known about one control apart from its name.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ControlSpec {
    /// Lowest legal value: 0 for a one-sided control, −1 for a signed one.
    pub lo: f32,
    /// Highest legal value, always 1.
    pub hi: f32,
    /// The value that leaves the face neutral. Always 0 today.
    pub rest: f32,
    pub spring: Spring,
}

impl ControlSpec {
    /// Whether the control has a negative side (`jaw_side`, `tongue_curl`,
    /// the head angles).
    pub fn signed(&self) -> bool {
        self.lo < 0.0
    }

    pub fn clamp(&self, v: f32) -> f32 {
        if v.is_nan() {
            self.rest
        } else {
            v.clamp(self.lo, self.hi)
        }
    }
}

macro_rules! controls {
    ($(
        $(#[$doc:meta])*
        $variant:ident = $name:literal, range($lo:literal, $hi:literal),
            rest $rest:literal, attack $attack:literal, release $release:literal;
    )*) => {
        /// One rig control. See the module docs: the order and the names are
        /// frozen (append-only), because the names become model floats.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        pub enum FaceControl {
            $( $(#[$doc])* #[serde(rename = $name)] $variant, )*
        }

        impl FaceControl {
            /// Every control, in declaration (= index = float) order.
            pub const ALL: &'static [FaceControl] = &[$(FaceControl::$variant),*];

            /// The stable snake_case name, which is also the model float's
            /// name. Never renamed.
            pub const fn name(self) -> &'static str {
                match self { $(FaceControl::$variant => $name,)* }
            }

            pub const fn spec(self) -> ControlSpec {
                match self {
                    $(FaceControl::$variant => ControlSpec {
                        lo: $lo,
                        hi: $hi,
                        rest: $rest,
                        spring: Spring { attack: $attack, release: $release },
                    },)*
                }
            }
        }
    };
}

controls! {
    /// Jaw rotation about the hinge: 0 shut, 1 fully open.
    JawOpen = "jaw_open", range(0.0, 1.0), rest 0.0, attack 38.0, release 30.0;
    /// Sideways jaw shift, creature's left (+x) positive.
    JawSide = "jaw_side", range(-1.0, 1.0), rest 0.0, attack 30.0, release 25.0;
    /// Lips pressed together regardless of the jaw (p, b, m).
    LipClose = "lip_close", range(0.0, 1.0), rest 0.0, attack 85.0, release 55.0;
    /// Lips flared forward and rounded open (o, sh).
    LipFunnel = "lip_funnel", range(0.0, 1.0), rest 0.0, attack 50.0, release 40.0;
    /// Lips pushed forward and narrowed tight (u, w).
    LipPucker = "lip_pucker", range(0.0, 1.0), rest 0.0, attack 50.0, release 40.0;
    /// Mouth corners pulled outward and back (i, e, s).
    LipStretch = "lip_stretch", range(0.0, 1.0), rest 0.0, attack 50.0, release 40.0;
    /// Upper lip raised, showing the upper teeth.
    LipUpperUp = "lip_upper_up", range(0.0, 1.0), rest 0.0, attack 45.0, release 35.0;
    /// Lower lip pulled down, showing the lower teeth.
    LipLowerDown = "lip_lower_down", range(0.0, 1.0), rest 0.0, attack 45.0, release 35.0;
    /// Corners up (a smile, or a snarl's start).
    LipCornerUp = "lip_corner_up", range(0.0, 1.0), rest 0.0, attack 30.0, release 22.0;
    /// Corners down (a frown, a sneer).
    LipCornerDown = "lip_corner_down", range(0.0, 1.0), rest 0.0, attack 30.0, release 22.0;
    /// Tongue pushed forward, past the teeth and out of the mouth.
    TongueOut = "tongue_out", range(0.0, 1.0), rest 0.0, attack 60.0, release 50.0;
    /// Tongue tip raised to the ridge behind the upper teeth (t, d, n, l).
    TongueUp = "tongue_up", range(0.0, 1.0), rest 0.0, attack 110.0, release 80.0;
    /// Tongue tip curled: + up and back (retroflex r), − down behind the
    /// lower teeth.
    TongueCurl = "tongue_curl", range(-1.0, 1.0), rest 0.0, attack 110.0, release 80.0;
    /// Cheeks inflated.
    CheekPuff = "cheek_puff", range(0.0, 1.0), rest 0.0, attack 25.0, release 18.0;
    /// Nostrils widened (a breath in, effort).
    NostrilFlare = "nostril_flare", range(0.0, 1.0), rest 0.0, attack 30.0, release 15.0;
    /// Brows raised.
    BrowUp = "brow_up", range(0.0, 1.0), rest 0.0, attack 22.0, release 14.0;
    /// Brows drawn down and together.
    BrowFurrow = "brow_furrow", range(0.0, 1.0), rest 0.0, attack 22.0, release 14.0;
    /// Lids opened wide.
    EyeWide = "eye_wide", range(0.0, 1.0), rest 0.0, attack 28.0, release 16.0;
    /// Both lids shut.
    Blink = "blink", range(0.0, 1.0), rest 0.0, attack 90.0, release 40.0;
    /// Lower lids raised, upper lowered: the whisper's and the sneer's eyes.
    Squint = "squint", range(0.0, 1.0), rest 0.0, attack 20.0, release 14.0;
    /// Head nod, + looks down.
    HeadPitch = "head_pitch", range(-1.0, 1.0), rest 0.0, attack 8.0, release 6.0;
    /// Head turn, + toward the creature's left (+x).
    HeadYaw = "head_yaw", range(-1.0, 1.0), rest 0.0, attack 6.0, release 5.0;
    /// Head tilt about the view axis.
    HeadRoll = "head_roll", range(-1.0, 1.0), rest 0.0, attack 6.0, release 5.0;

    // ---- 2026-10-07: the expression muscles (FACS), appended (rule 2) ----
    //
    // The 23 above are what speech needs, and they are symmetric. A gesture
    // is not: a wink, a smirk, a sneer and a raised eyebrow are one side
    // only. Each per-side control *adds* to its symmetric sibling (clamped),
    // so a viseme still drives both corners and a smirk rides on top of it.
    // "l" is the creature's own left (+x, the viewer's right).

    /// AU1, inner brow raiser: the inner ends go up and the brows tilt into a
    /// worried / pleading / terrified line. `brow_up` lifts the whole brow.
    BrowInnerUp = "brow_inner_up", range(0.0, 1.0), rest 0.0, attack 22.0, release 14.0;
    /// AU2, outer brow raiser, left only: the sceptical eyebrow.
    BrowOuterUpL = "brow_outer_up_l", range(0.0, 1.0), rest 0.0, attack 22.0, release 14.0;
    /// AU2, outer brow raiser, right only.
    BrowOuterUpR = "brow_outer_up_r", range(0.0, 1.0), rest 0.0, attack 22.0, release 14.0;
    /// AU4, brow lowerer (corrugator), left only: down and in.
    BrowDownL = "brow_down_l", range(0.0, 1.0), rest 0.0, attack 22.0, release 14.0;
    /// AU4, brow lowerer, right only.
    BrowDownR = "brow_down_r", range(0.0, 1.0), rest 0.0, attack 22.0, release 14.0;
    /// AU5, upper-lid raiser, left only (adds to `eye_wide`).
    EyeWideL = "eye_wide_l", range(0.0, 1.0), rest 0.0, attack 28.0, release 16.0;
    /// AU5, upper-lid raiser, right only.
    EyeWideR = "eye_wide_r", range(0.0, 1.0), rest 0.0, attack 28.0, release 16.0;
    /// AU46, the left lid shut on its own: a wink (adds to `blink`).
    BlinkL = "blink_l", range(0.0, 1.0), rest 0.0, attack 90.0, release 40.0;
    /// AU46, the right lid shut on its own.
    BlinkR = "blink_r", range(0.0, 1.0), rest 0.0, attack 90.0, release 40.0;
    /// AU6, cheek raiser, left: the cheek climbs, the lower lid bunches up
    /// under the eye — what makes a smile reach the eyes.
    CheekRaiseL = "cheek_raise_l", range(0.0, 1.0), rest 0.0, attack 25.0, release 18.0;
    /// AU6, cheek raiser, right.
    CheekRaiseR = "cheek_raise_r", range(0.0, 1.0), rest 0.0, attack 25.0, release 18.0;
    /// AU9, nose wrinkler, left: the nostril wing hikes up and back, the
    /// upper lip and cheek follow, the inner brow is dragged down. The sneer.
    NoseWrinkleL = "nose_wrinkle_l", range(0.0, 1.0), rest 0.0, attack 30.0, release 18.0;
    /// AU9, nose wrinkler, right.
    NoseWrinkleR = "nose_wrinkle_r", range(0.0, 1.0), rest 0.0, attack 30.0, release 18.0;
    /// AU10, upper-lip raiser, left half only: the snarl that bares one fang
    /// (adds to `lip_upper_up`).
    LipUpperUpL = "lip_upper_up_l", range(0.0, 1.0), rest 0.0, attack 45.0, release 35.0;
    /// AU10, upper-lip raiser, right half only.
    LipUpperUpR = "lip_upper_up_r", range(0.0, 1.0), rest 0.0, attack 45.0, release 35.0;
    /// AU12, lip-corner puller, left: the corner goes up *and back* (adds to
    /// `lip_corner_up`). One side alone is a smirk.
    SmileL = "smile_l", range(0.0, 1.0), rest 0.0, attack 30.0, release 22.0;
    /// AU12, lip-corner puller, right.
    SmileR = "smile_r", range(0.0, 1.0), rest 0.0, attack 30.0, release 22.0;
    /// AU15, lip-corner depressor, left (adds to `lip_corner_down`).
    FrownL = "frown_l", range(0.0, 1.0), rest 0.0, attack 30.0, release 22.0;
    /// AU15, lip-corner depressor, right.
    FrownR = "frown_r", range(0.0, 1.0), rest 0.0, attack 30.0, release 22.0;
    /// AU20, lip stretcher, left: the corner pulled straight out toward the
    /// ear — the grimace of fear (adds to `lip_stretch`).
    LipStretchL = "lip_stretch_l", range(0.0, 1.0), rest 0.0, attack 50.0, release 40.0;
    /// AU20, lip stretcher, right.
    LipStretchR = "lip_stretch_r", range(0.0, 1.0), rest 0.0, attack 50.0, release 40.0;
    /// AU24, lip presser: the lips squeezed thin and tight against each
    /// other — anger held in. Unlike `lip_close` it thins the lips and
    /// pulls the corners in, and on a shut mouth it is visible.
    LipPress = "lip_press", range(0.0, 1.0), rest 0.0, attack 60.0, release 40.0;
}

impl FaceControl {
    /// How many controls there are, derived from [`FaceControl::ALL`].
    pub const COUNT: usize = Self::ALL.len();

    /// Index into [`FaceControl::ALL`] and into [`Controls`].
    pub const fn index(self) -> usize {
        self as usize
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|c| c.name() == name)
    }

    /// The jaw, lips and tongue: the controls speech articulates, as opposed
    /// to the upper face and the head. Style changes these springs.
    pub fn is_mouth(self) -> bool {
        use FaceControl::*;
        matches!(
            self,
            JawOpen
                | JawSide
                | LipClose
                | LipFunnel
                | LipPucker
                | LipStretch
                | LipUpperUp
                | LipLowerDown
                | LipCornerUp
                | LipCornerDown
                | TongueOut
                | TongueUp
                | TongueCurl
                | LipUpperUpL
                | LipUpperUpR
                | SmileL
                | SmileR
                | FrownL
                | FrownR
                | LipStretchL
                | LipStretchR
                | LipPress
        )
    }
}

/// A value for every control, indexed by [`FaceControl`].
///
/// Serialized as a map from control name to value, not as an array: a saved
/// value written before a control was appended loads with that control at
/// rest, and a name this build does not know is ignored rather than failing
/// the whole load.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Controls(pub [f32; FaceControl::COUNT]);

impl Controls {
    /// Every control at its rest value.
    pub fn rest() -> Self {
        let mut v = [0.0; FaceControl::COUNT];
        for c in FaceControl::ALL {
            v[c.index()] = c.spec().rest;
        }
        Controls(v)
    }

    /// Rest, with the listed controls set (and clamped).
    pub fn with(pairs: &[(FaceControl, f32)]) -> Self {
        let mut out = Self::rest();
        for &(c, v) in pairs {
            out.set(c, v);
        }
        out
    }

    pub fn get(&self, c: FaceControl) -> f32 {
        self.0[c.index()]
    }

    /// Sets a control, clamped to its range.
    pub fn set(&mut self, c: FaceControl, v: f32) {
        self.0[c.index()] = c.spec().clamp(v);
    }

    /// Every value clamped to its control's range.
    pub fn clamped(mut self) -> Self {
        for c in FaceControl::ALL {
            self.0[c.index()] = c.spec().clamp(self.0[c.index()]);
        }
        self
    }

    pub fn iter(&self) -> impl Iterator<Item = (FaceControl, f32)> + '_ {
        FaceControl::ALL.iter().map(|&c| (c, self.get(c)))
    }
}

impl Default for Controls {
    fn default() -> Self {
        Self::rest()
    }
}

impl Index<FaceControl> for Controls {
    type Output = f32;
    fn index(&self, c: FaceControl) -> &f32 {
        &self.0[c.index()]
    }
}

impl IndexMut<FaceControl> for Controls {
    fn index_mut(&mut self, c: FaceControl) -> &mut f32 {
        &mut self.0[c.index()]
    }
}

impl Serialize for Controls {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut map = s.serialize_map(Some(FaceControl::COUNT))?;
        for (c, v) in self.iter() {
            map.serialize_entry(c.name(), &v)?;
        }
        map.end()
    }
}

impl<'de> Deserialize<'de> for Controls {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct ControlsVisitor;
        impl<'de> Visitor<'de> for ControlsVisitor {
            type Value = Controls;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a map from face control name to value")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Controls, A::Error> {
                let mut out = Controls::rest();
                while let Some((name, v)) = map.next_entry::<String, f32>()? {
                    if let Some(c) = FaceControl::from_name(&name) {
                        out.set(c, v);
                    }
                }
                Ok(out)
            }
        }
        d.deserialize_map(ControlsVisitor)
    }
}
