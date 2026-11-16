//! Visemes: the mouth shapes speech is drawn with, as poses over the
//! controls.
//!
//! The set is the common lip-sync one (Oculus/ARKit-style): silence, ten
//! consonant classes and five vowels — fourteen entries. A viseme is not a
//! phoneme. p, b and m look the same from outside, so they are one viseme;
//! a mouth only has to be **plausible**, not a transcript (decision 8).
//!
//! Each pose is a short list of the controls it moves; everything it does not
//! name stays at rest. That keeps the table readable and makes `sil` exactly
//! the rest pose.
//!
//! [`blend`] mixes poses by weight. Weights that sum to less than 1 leave the
//! remainder at rest — a faint viseme is a small mouth movement, not a
//! normalized full one — and weights that sum to more than 1 are normalized,
//! so two strong guesses average rather than add.

use serde::{Deserialize, Serialize};

use crate::control::{Controls, FaceControl as C, FaceControl};

/// One mouth shape. Names are the usual ones, kept verbatim (`sil`, `PP`,
/// `aa`…), because they are what every lip-sync tool and paper calls them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Viseme {
    #[serde(rename = "sil")]
    Sil,
    /// p, b, m: lips pressed.
    PP,
    /// f, v: lower lip to the upper teeth.
    FF,
    /// th: tongue between the teeth.
    TH,
    /// t, d, n, l: tongue tip on the ridge.
    DD,
    /// k, g: back of the tongue up, jaw a little open.
    #[serde(rename = "kk")]
    Kk,
    /// ch, sh, j: lips rounded forward, teeth near.
    CH,
    /// s, z: teeth together, lips spread.
    SS,
    /// r: lips a little round, tongue curled.
    RR,
    /// a: jaw wide open.
    #[serde(rename = "aa")]
    Aa,
    E,
    I,
    O,
    U,
}

impl Viseme {
    pub const ALL: &'static [Viseme] = &[
        Viseme::Sil,
        Viseme::PP,
        Viseme::FF,
        Viseme::TH,
        Viseme::DD,
        Viseme::Kk,
        Viseme::CH,
        Viseme::SS,
        Viseme::RR,
        Viseme::Aa,
        Viseme::E,
        Viseme::I,
        Viseme::O,
        Viseme::U,
    ];

    pub const COUNT: usize = Self::ALL.len();

    pub const fn name(self) -> &'static str {
        match self {
            Viseme::Sil => "sil",
            Viseme::PP => "PP",
            Viseme::FF => "FF",
            Viseme::TH => "TH",
            Viseme::DD => "DD",
            Viseme::Kk => "kk",
            Viseme::CH => "CH",
            Viseme::SS => "SS",
            Viseme::RR => "RR",
            Viseme::Aa => "aa",
            Viseme::E => "E",
            Viseme::I => "I",
            Viseme::O => "O",
            Viseme::U => "U",
        }
    }

    /// The controls this viseme moves away from rest.
    pub fn moves(self) -> &'static [(FaceControl, f32)] {
        match self {
            Viseme::Sil => &[],
            Viseme::PP => &[(C::LipClose, 1.0), (C::JawOpen, 0.05)],
            // No control tucks the lower lip under the teeth directly: a
            // partial close with the upper lip raised reads as it.
            Viseme::FF => &[
                (C::JawOpen, 0.08),
                (C::LipUpperUp, 0.35),
                (C::LipClose, 0.3),
                (C::LipStretch, 0.2),
            ],
            Viseme::TH => &[(C::JawOpen, 0.2), (C::TongueOut, 0.3), (C::TongueUp, 0.3)],
            Viseme::DD => &[(C::JawOpen, 0.2), (C::TongueUp, 1.0), (C::LipStretch, 0.1)],
            Viseme::Kk => &[
                (C::JawOpen, 0.3),
                (C::TongueUp, 0.2),
                (C::TongueCurl, -0.3),
                (C::LipStretch, 0.15),
            ],
            Viseme::CH => &[
                (C::JawOpen, 0.15),
                (C::LipFunnel, 0.6),
                (C::LipPucker, 0.2),
                (C::LipUpperUp, 0.2),
                (C::TongueUp, 0.5),
            ],
            Viseme::SS => &[
                (C::JawOpen, 0.08),
                (C::LipStretch, 0.45),
                (C::LipUpperUp, 0.25),
                (C::LipLowerDown, 0.2),
                (C::TongueUp, 0.6),
            ],
            Viseme::RR => &[
                (C::JawOpen, 0.2),
                (C::LipFunnel, 0.35),
                (C::LipPucker, 0.2),
                (C::TongueCurl, 0.7),
                (C::TongueUp, 0.3),
            ],
            Viseme::Aa => &[
                (C::JawOpen, 0.85),
                (C::LipLowerDown, 0.2),
                (C::LipUpperUp, 0.1),
            ],
            Viseme::E => &[
                (C::JawOpen, 0.4),
                (C::LipStretch, 0.45),
                (C::LipCornerUp, 0.15),
            ],
            Viseme::I => &[
                (C::JawOpen, 0.2),
                (C::LipStretch, 0.75),
                (C::LipCornerUp, 0.25),
                (C::LipUpperUp, 0.15),
            ],
            Viseme::O => &[(C::JawOpen, 0.5), (C::LipFunnel, 0.75), (C::LipPucker, 0.3)],
            Viseme::U => &[(C::JawOpen, 0.2), (C::LipPucker, 0.9), (C::LipFunnel, 0.4)],
        }
    }

    /// The full control pose: rest plus [`Viseme::moves`].
    pub fn pose(self) -> Controls {
        Controls::with(self.moves())
    }
}

/// Mixes viseme poses by weight; see the module docs. Negative and non-finite
/// weights count as 0. The result is clamped to every control's range.
pub fn blend(weights: &[(Viseme, f32)]) -> Controls {
    let rest = Controls::rest();
    let w = |x: f32| if x.is_finite() { x.max(0.0) } else { 0.0 };
    let total: f32 = weights.iter().map(|&(_, x)| w(x)).sum();
    let norm = if total > 1.0 { 1.0 / total } else { 1.0 };
    let mut out = rest;
    for &(v, x) in weights {
        let x = w(x) * norm;
        if x == 0.0 {
            continue;
        }
        let p = v.pose();
        for c in FaceControl::ALL {
            out[*c] += x * (p[*c] - rest[*c]);
        }
    }
    out.clamped()
}
