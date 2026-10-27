//! `metal-rig` as an [`AuxLogic`] — the reference implementation of
//! `docs/plan-architecture.md` D1.
//!
//! The rig has always been "CPU logic that runs before a frame is drawn": it
//! takes rotations and produces a resolved body, reading no pixels and writing
//! no pixels. Before `metal-logic` it did this through its own API and its own
//! conventions; here it is a value of a generic trait, so a script runtime
//! (`O.6`/`S.G`) can be written against the same boundary rather than a second
//! one shaped like the rig.
//!
//! The row in is a pose tensor ([`Pose::tensor_row`], `joints × 3 + 3`). The
//! row out is a small declared control row — root, centre of mass, grounded —
//! which is what a downstream stage (a `Mask`, a light cue) can read without
//! knowing the rig's internals.

use metal_logic::{AuxError, AuxInput, AuxLogic, AuxOutput, FloatDecl, LogicContext};

use crate::contact::Contact;
use crate::frame::tensor_width;
use crate::pose::Pose;
use crate::rig;
use crate::skeleton::Skeleton;

/// The floats [`RigLogic`] writes, in row order. Declared here, next to the
/// code that fills them, so a script that reads them can be validated against
/// the same list the rig writes.
pub static RIG_FLOATS: &[FloatDecl] = &[
    FloatDecl::new("root_x", -2.0, 2.0, 0.0),
    FloatDecl::new("root_y", -2.0, 2.0, 0.0),
    FloatDecl::new("root_z", -2.0, 2.0, 0.0),
    FloatDecl::new("com_x", -2.0, 2.0, 0.0),
    FloatDecl::new("com_y", -2.0, 2.0, 0.0),
    FloatDecl::new("com_z", -2.0, 2.0, 0.0),
    // 1 when the body is on the floor, 0 when it is airborne. Read off the
    // contact tracks, which is the rig's own notion of "grounded" rather than
    // a second one invented here.
    FloatDecl::new("grounded", 0.0, 1.0, 0.0),
];

/// The rig as a piece of [`AuxLogic`].
///
/// Holds the skeleton it resolves against and the contact set it reads
/// "grounded" from. `rig::canonical()` is the humanoid default; LAY.12 turns
/// the skeleton itself into data (`RigSpec`) so this type does not assume a
/// humanoid shape.
pub struct RigLogic {
    skeleton: Skeleton,
    contact: Contact,
}

impl RigLogic {
    /// The canonical humanoid rig, standing on the floor.
    pub fn humanoid() -> RigLogic {
        let skeleton = rig::canonical();
        let contact = Contact::standing(&skeleton);
        RigLogic { skeleton, contact }
    }

    /// The rig for an already-built skeleton, standing on the floor. The door
    /// LAY.12's non-humanoid spec will use.
    pub fn with_skeleton(skeleton: Skeleton) -> RigLogic {
        let contact = Contact::standing(&skeleton);
        RigLogic { skeleton, contact }
    }

    pub fn skeleton(&self) -> &Skeleton {
        &self.skeleton
    }
}

impl AuxLogic for RigLogic {
    fn id(&self) -> &'static str {
        "rig"
    }

    fn floats(&self) -> &'static [FloatDecl] {
        RIG_FLOATS
    }

    fn input_width(&self) -> usize {
        tensor_width(self.skeleton.len())
    }

    fn run(&mut self, input: &AuxInput, ctx: &LogicContext) -> Result<AuxOutput, AuxError> {
        let expected = self.input_width();
        if input.row.len() != expected {
            return Err(AuxError::BadInput(format!(
                "rig expects a {expected}-float pose tensor, got {}",
                input.row.len()
            )));
        }
        // Any non-finite entry would silently poison every derived number; a
        // NaN pose is bad input, reported as such.
        if input.row.iter().any(|v| !v.is_finite()) {
            return Err(AuxError::BadInput("rig input row has a non-finite value".into()));
        }

        let pose = Pose::from_tensor_row(&self.skeleton, input.row);
        let points = pose.points(&self.skeleton);
        let n = points.len().max(1) as f32;
        let com = points.iter().fold(glam::Vec3::ZERO, |a, p| a + *p) / n;
        // "Grounded" is whether anything is touching, not how much load was
        // annotated: `Contact::standing` declares the soles touching with no
        // load curve, and reading load would call a standing body airborne.
        let grounded = if self.contact.touching_at(ctx.time).is_empty() { 0.0 } else { 1.0 };

        Ok(AuxOutput::new(vec![
            pose.root.x,
            pose.root.y,
            pose.root.z,
            com.x,
            com.y,
            com.z,
            grounded,
        ]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use metal_logic::run_with_budget;
    use metal_logic::Budget;

    fn ctx() -> LogicContext {
        LogicContext::new(0.0, 1.0 / 60.0, 0, Budget::DEFAULT)
    }

    #[test]
    fn the_rig_runs_through_the_trait_not_its_internal_api() {
        // The point of LAY.11: the test holds a `dyn AuxLogic`, not a
        // `RigLogic` method, and the contract supplies the rest.
        let skeleton = rig::canonical();
        let rest = Pose::rest(&skeleton);
        let mut row = vec![0.0f32; tensor_width(skeleton.len())];
        rest.tensor_row(&mut row);

        let mut logic: Box<dyn AuxLogic> = Box::new(RigLogic::with_skeleton(skeleton));
        assert_eq!(logic.id(), "rig");
        assert_eq!(logic.floats().len(), RIG_FLOATS.len());
        assert_eq!(logic.input_width(), row.len());

        let out = run_with_budget(&mut *logic, &AuxInput::new(&row), &ctx()).unwrap();
        assert_eq!(out.row.len(), RIG_FLOATS.len());
        // The rest pose is grounded, so the derived flag is on.
        assert!(out.row[6] > 0.0);
    }

    #[test]
    fn a_short_row_is_a_normal_error() {
        let mut logic = RigLogic::humanoid();
        let err = run_with_budget(&mut logic, &AuxInput::new(&[0.0; 3]), &ctx()).unwrap_err();
        assert!(matches!(err, AuxError::BadInput(_)), "{err:?}");
    }

    #[test]
    fn a_nan_pose_is_a_normal_error_not_a_poisoned_row() {
        let mut logic = RigLogic::humanoid();
        let mut row = vec![0.0f32; logic.input_width()];
        row[0] = f32::NAN;
        let err = run_with_budget(&mut logic, &AuxInput::new(&row), &ctx()).unwrap_err();
        assert!(matches!(err, AuxError::BadInput(_)), "{err:?}");
    }

    /// LAY.12: the contract does not assume a humanoid — no laterality, no
    /// height, no sole. A three-joint arm runs through the *same* trait and
    /// the *same* code path, and is simply not grounded because it has no
    /// contact joints. This is the verification the plan asks for, not a
    /// rewrite of the rig.
    #[test]
    fn the_same_contract_runs_a_skeleton_that_is_not_human() {
        use crate::{Joint, Side, Skeleton};
        let skeleton = Skeleton {
            name: "arm".into(),
            joints: vec![
                Joint {
                    name: "shoulder".into(),
                    parent: None,
                    side: Side::Center,
                    rest_offset: glam::Vec3::ZERO,
                    rest_rotation: glam::Quat::IDENTITY,
                },
                Joint {
                    name: "elbow".into(),
                    parent: Some(0),
                    side: Side::Center,
                    rest_offset: glam::Vec3::new(0.0, -0.3, 0.0),
                    rest_rotation: glam::Quat::IDENTITY,
                },
                Joint {
                    name: "hand".into(),
                    parent: Some(1),
                    side: Side::Center,
                    rest_offset: glam::Vec3::new(0.0, -0.3, 0.0),
                    rest_rotation: glam::Quat::IDENTITY,
                },
            ],
        };
        skeleton.validate().expect("a valid three-joint skeleton");

        let mut logic: Box<dyn AuxLogic> = Box::new(RigLogic::with_skeleton(skeleton));
        assert_eq!(logic.input_width(), 3 * 3 + 3, "3 joints x 3 + root");
        let row = vec![0.0f32; logic.input_width()];
        let out = run_with_budget(&mut *logic, &AuxInput::new(&row), &ctx()).unwrap();
        assert_eq!(out.row.len(), RIG_FLOATS.len());
        // No sole names anywhere in it, so the same grounded channel simply
        // reads 0 — no special case, no humanoid assumption.
        assert_eq!(out.row[6], 0.0);
    }
}
