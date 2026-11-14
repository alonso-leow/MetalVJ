//! The five species, as proportions.
//!
//! A species is a handful of numbers, never a different face. Every species
//! has the same landmarks in the same order — one jaw, one lip ring, one
//! tongue chain — and [`crate::Topology::for_species`] only moves and scales
//! them. That is decision 10's whole point: the rig, the visemes and the
//! springs are written once and work on a dragon and a minotaur alike, and a
//! sixth species is a sixth row in [`Kind::species`], not new rig code.
//!
//! Some parameters change the landmarks (snout, jaw width, brow ridge, eye
//! spacing, fang length, tongue length). Others only reach the shader's skull
//! (horns, scales, the forked tongue): they ride along in
//! [`crate::Posed::pack`]'s `shape` section so the SDF can read them without
//! a second uniform.

use serde::{Deserialize, Serialize};

/// The proportions of one species. Every field has a neutral value
/// ([`Species::NEUTRAL`]) at which the topology is exactly
/// [`crate::Topology::neutral`].
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Species {
    /// How far the muzzle pushes the mouth forward, 0 (a flat face) … 1 (a
    /// crocodile). Deepens the mouth cavity with it.
    pub snout: f32,
    /// Mouth, teeth, cavity and cheek width as a factor of neutral.
    pub jaw_width: f32,
    /// Brow ridge, 0 … 1: brows up and forward, eyes sunk under them.
    pub brow_ridge: f32,
    /// Horn amount for the shader's skull, 0 … 1. No landmark moves.
    pub horns: f32,
    /// Scale amount for the shader's skin, 0 … 1. No landmark moves.
    pub scales: f32,
    /// Distance between the eyes as a factor of neutral.
    pub eye_spacing: f32,
    /// Canine length beyond the other teeth, 0 … 1 (upper fangs down, lower
    /// tusks up).
    pub fang_length: f32,
    /// Tongue length as a factor of neutral. The resting tongue always fits
    /// the cavity; length shows as reach when `tongue_out` pushes it out.
    pub tongue_length: f32,
    /// A split tip, drawn by the shader. The rig is the same chain either way.
    pub forked_tongue: bool,
    /// Gain on the appended expression muscles' displacements (brows,
    /// cheeks, corners, snarl): a scaly or heavy-skulled species needs a
    /// bigger push for a one-sided gesture to read at thumbnail size. 1 on
    /// a smooth face. Absent in older saved species → 1.
    #[serde(default = "unit_gain")]
    pub expression: f32,
}

fn unit_gain() -> f32 {
    1.0
}

impl Species {
    /// The proportions [`crate::Topology::neutral`] is built from.
    pub const NEUTRAL: Species = Species {
        snout: 0.0,
        jaw_width: 1.0,
        brow_ridge: 0.0,
        horns: 0.0,
        scales: 0.0,
        eye_spacing: 1.0,
        fang_length: 0.0,
        tongue_length: 1.0,
        forked_tongue: false,
        expression: 1.0,
    };
}

impl Default for Species {
    fn default() -> Self {
        Self::NEUTRAL
    }
}

/// The species `monster` ships with. The order is the `especie` float's
/// index order, so it is append-only like everything else that becomes a
/// float.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Dragon,
    Minotauro,
    Gorgona,
    Oni,
    Quetzalcoatl,
}

impl Kind {
    pub const ALL: &'static [Kind] = &[
        Kind::Dragon,
        Kind::Minotauro,
        Kind::Gorgona,
        Kind::Oni,
        Kind::Quetzalcoatl,
    ];

    /// Stable ASCII identifier.
    pub const fn name(self) -> &'static str {
        match self {
            Kind::Dragon => "dragon",
            Kind::Minotauro => "minotauro",
            Kind::Gorgona => "gorgona",
            Kind::Oni => "oni",
            Kind::Quetzalcoatl => "quetzalcoatl",
        }
    }

    /// Display name, as the operator wrote them.
    pub const fn label(self) -> &'static str {
        match self {
            Kind::Dragon => "dragon",
            Kind::Minotauro => "minotaur",
            Kind::Gorgona => "gorgon",
            Kind::Oni => "oni",
            Kind::Quetzalcoatl => "quetzalcoatl",
        }
    }

    /// The species for an `especie` float value: rounded and clamped, so any
    /// value a slider can reach names a species.
    pub fn from_float(v: f32) -> Kind {
        let last = Self::ALL.len() - 1;
        let i = if v.is_finite() {
            v.round().clamp(0.0, last as f32) as usize
        } else {
            0
        };
        Self::ALL[i]
    }

    pub fn species(self) -> Species {
        match self {
            // Long muzzle, wide gape, heavy ridge, horns and scales, a long
            // forked tongue and real fangs.
            Kind::Dragon => Species {
                snout: 0.8,
                jaw_width: 1.2,
                brow_ridge: 0.7,
                horns: 0.8,
                scales: 1.0,
                eye_spacing: 1.15,
                fang_length: 0.7,
                tongue_length: 1.4,
                forked_tongue: true,
                expression: 1.9,
            },
            // A bull's muzzle: medium snout, the widest jaw, full horns, flat
            // herbivore teeth.
            Kind::Minotauro => Species {
                snout: 0.5,
                jaw_width: 1.3,
                brow_ridge: 0.6,
                horns: 1.0,
                scales: 0.0,
                eye_spacing: 1.2,
                fang_length: 0.0,
                tongue_length: 1.0,
                forked_tongue: false,
                expression: 1.8,
            },
            // Nearly human face, snake's mouth: narrow jaw, fangs, a forked
            // tongue. The hair of snakes is the shader's `horns`.
            Kind::Gorgona => Species {
                snout: 0.1,
                jaw_width: 0.9,
                brow_ridge: 0.3,
                horns: 0.3,
                scales: 0.6,
                eye_spacing: 0.95,
                fang_length: 0.6,
                tongue_length: 1.3,
                forked_tongue: true,
                expression: 1.0,
            },
            // A flat, wide demon face: the heaviest brow, horns, tusks.
            Kind::Oni => Species {
                snout: 0.15,
                jaw_width: 1.25,
                brow_ridge: 1.0,
                horns: 0.7,
                scales: 0.0,
                eye_spacing: 1.05,
                fang_length: 0.9,
                tongue_length: 1.1,
                forked_tongue: false,
                expression: 1.1,
            },
            // The feathered serpent: the longest muzzle and tongue, scales,
            // feathers (the shader's `horns`, a little).
            Kind::Quetzalcoatl => Species {
                snout: 0.9,
                jaw_width: 1.0,
                brow_ridge: 0.4,
                horns: 0.2,
                scales: 0.9,
                eye_spacing: 1.2,
                fang_length: 0.5,
                tongue_length: 1.5,
                forked_tongue: true,
                expression: 1.8,
            },
        }
    }
}
