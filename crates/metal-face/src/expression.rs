//! Whole-face expressions as control presets: the gestures the rig was
//! extended for (2026-10-07), as numbers an author, a loop template or a test
//! can start from.
//!
//! An expression is a sparse list of `(control, value)`; everything it does
//! not name stays at rest, so it composes with a viseme (the mouth can still
//! sing over a sneer) and with the head angles. The values are the ones the
//! face-shot proof sheet renders; they were tuned by looking at those frames
//! on all five species, not derived from anything.
//!
//! | expression | what sells it |
//! |---|---|
//! | `rage` | both brows down and in (AU4), nose wrinkled (AU9), both upper lips up (AU10) baring the fangs, corners stretched (AU20), jaw half open, nostrils flared, lids weighed down by the brows |
//! | `terror` | inner brows up (AU1) with the whole brow up (AU1+2), lids wide (AU5), corners stretched flat toward the ears (AU20), jaw dropped, lower lip down |
//! | `sneer` | one side only: nose wrinkle + upper lip up on the left (AU9+10), that brow down, cheek up, the head tipped away |
//! | `wink` | left lid shut (AU46), left cheek raised (AU6), a lopsided smile (AU12), the open eye's brow up |
//! | `smirk` | one corner up and back (AU12 left), lips pressed (AU24), the opposite brow tail raised (AU2 right) |
//! | `surprise` | brows up (AU1+2), lids wide (AU5), jaw dropped (AU26), lips loosely rounded |
//! | `joy` | both corners up and back (AU12), cheeks raised (AU6) so the lower lids bunch, upper teeth showing |
//! | `grief` | inner brows up (AU1), corners down (AU15), lips pressed (AU24), lids heavy |

use crate::control::{Controls, FaceControl as C};

/// One named expression: the controls it sets, everything else at rest.
#[derive(Clone, Copy, Debug)]
pub struct Expression {
    pub name: &'static str,
    pub controls: &'static [(C, f32)],
}

impl Expression {
    /// The expression as a full control value (unnamed controls at rest).
    pub fn controls(&self) -> Controls {
        Controls::with(self.controls)
    }

    pub fn by_name(name: &str) -> Option<&'static Expression> {
        EXPRESSIONS.iter().find(|e| e.name == name)
    }
}

/// The preset table. See the module docs for what each one is made of.
pub const EXPRESSIONS: &[Expression] = &[
    Expression {
        name: "rage",
        controls: &[
            (C::BrowDownL, 1.0),
            (C::BrowDownR, 1.0),
            (C::BrowFurrow, 0.6),
            (C::NoseWrinkleL, 0.8),
            (C::NoseWrinkleR, 0.8),
            (C::LipUpperUpL, 0.9),
            (C::LipUpperUpR, 0.9),
            (C::LipStretchL, 0.6),
            (C::LipStretchR, 0.6),
            (C::LipLowerDown, 0.6),
            (C::JawOpen, 0.45),
            (C::NostrilFlare, 1.0),
            (C::Squint, 0.25),
            (C::HeadPitch, 0.2),
        ],
    },
    Expression {
        name: "terror",
        controls: &[
            (C::BrowInnerUp, 1.0),
            (C::BrowUp, 0.7),
            (C::EyeWide, 1.0),
            (C::LipStretchL, 1.0),
            (C::LipStretchR, 1.0),
            (C::LipLowerDown, 0.7),
            (C::JawOpen, 0.6),
            (C::NostrilFlare, 0.6),
            (C::HeadPitch, -0.15),
        ],
    },
    Expression {
        name: "sneer",
        controls: &[
            (C::NoseWrinkleL, 1.0),
            (C::LipUpperUpL, 1.0),
            (C::BrowDownL, 0.6),
            (C::CheekRaiseL, 0.5),
            (C::FrownR, 0.3),
            (C::JawOpen, 0.12),
            (C::HeadRoll, 0.2),
            (C::HeadYaw, -0.15),
            (C::HeadPitch, -0.1),
        ],
    },
    Expression {
        name: "wink",
        controls: &[
            (C::BlinkL, 1.0),
            (C::CheekRaiseL, 0.9),
            (C::SmileL, 0.9),
            (C::SmileR, 0.35),
            (C::BrowDownL, 0.35),
            (C::BrowOuterUpR, 0.5),
            (C::HeadRoll, -0.15),
        ],
    },
    Expression {
        name: "smirk",
        controls: &[
            (C::SmileL, 1.0),
            (C::CheekRaiseL, 0.4),
            (C::LipPress, 0.4),
            (C::BrowOuterUpR, 0.8),
            (C::Squint, 0.2),
            (C::HeadRoll, 0.12),
            (C::HeadYaw, 0.1),
        ],
    },
    Expression {
        name: "surprise",
        controls: &[
            (C::BrowUp, 1.0),
            (C::BrowInnerUp, 0.4),
            (C::EyeWide, 1.0),
            (C::JawOpen, 0.75),
            (C::LipFunnel, 0.35),
            (C::HeadPitch, -0.1),
        ],
    },
    Expression {
        name: "joy",
        controls: &[
            (C::SmileL, 1.0),
            (C::SmileR, 1.0),
            (C::CheekRaiseL, 0.9),
            (C::CheekRaiseR, 0.9),
            (C::LipUpperUp, 0.35),
            (C::JawOpen, 0.3),
            (C::BrowUp, 0.2),
        ],
    },
    Expression {
        name: "grief",
        controls: &[
            (C::BrowInnerUp, 1.0),
            (C::FrownL, 1.0),
            (C::FrownR, 1.0),
            (C::LipPress, 0.35),
            (C::Squint, 0.3),
            (C::HeadPitch, 0.25),
        ],
    },
];
