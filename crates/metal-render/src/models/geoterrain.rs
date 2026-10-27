//! Model 1 — Geoterrain. Parametric spiral arms: `r = radius * s`,
//! `theta = twist * s + arm_offset`, drawn as thick lines through the
//! shared `line2d` pipeline. The simplest model on purpose — it is the one
//! that validates the pipeline everything else in this file also uses.

use glam::Vec2;

use crate::model::{color, background, spec, Block, Built, Ctx, FloatSpec, Model, Schema};
use crate::shared::backdrop::Backdrop;
use crate::shared::line2d::{Line2dPipeline, Strip, Vertex};
use crate::shared::ColorSlotGpu;

const FLOATS: &[FloatSpec] = &[
    spec("radius", 0.05, 1.2, 0.55),
    spec("angular_speed", -6.0, 6.0, 1.1),
    spec("arms", 1.0, 8.0, 3.0),
    spec("thickness", 0.002, 0.05, 0.012),
    spec("twist", 2.0, 24.0, 9.0),
    spec("density", 16.0, 400.0, 180.0),
];

pub static SCHEMA: Schema = Schema {
    blocks: &[Block::Floats(FLOATS), color("spiral"), background(), crate::model::SIGNAL_FLOATS_BLOCK, crate::model::EFFECTS_FLOATS_BLOCK, crate::model::EFFECTS_COLOR_BLOCK],
};

pub struct Geoterrain {
    pipeline: Built<Line2dPipeline>,
    backdrop: Built<Backdrop>,
}

impl Geoterrain {
    pub fn new() -> Geoterrain {
        Geoterrain { pipeline: Built::default(), backdrop: Built::default() }
    }
}

impl Model for Geoterrain {
    fn id(&self) -> &'static str {
        "geoterrain"
    }

    fn name(&self) -> &'static str {
        "Geoterrain"
    }

    fn schema(&self) -> &'static Schema {
        &SCHEMA
    }

    fn backdrop(&self) -> bool {
        true
    }

    fn render(
        &mut self,
        ctx: &Ctx,
        floats: &[f32],
        target: &wgpu::TextureView,
        format: wgpu::TextureFormat,
    ) {
        let radius = floats[0];
        let angular_speed = floats[1];
        let arms = (floats[2].round() as u32).max(1);
        let thickness = floats[3].max(0.0005);
        let twist = floats[4];
        let density = (floats[5].round() as u32).clamp(2, 4000);
        let color = ColorSlotGpu::from_floats(&floats[6..18]);
        let background = ColorSlotGpu::from_floats(&floats[18..30]);

        // Before the lines, and it is the pass that clears — `line2d` loads.
        self.backdrop
            .get(format, || Backdrop::new(ctx.device, format))
            .draw(ctx, target, background, 0.0, ctx.fondo_alpha);
        let pipeline = self
            .pipeline
            .get(format, || Line2dPipeline::new(ctx.device, format));

        let mut vertices = Vec::with_capacity((arms * density * 2) as usize);
        let mut strips = Vec::with_capacity(arms as usize);

        for arm in 0..arms {
            let arm_offset =
                (arm as f32 / arms as f32) * std::f32::consts::TAU + ctx.time * angular_speed;
            let start = vertices.len() as u32;
            for i in 0..density {
                let s = i as f32 / (density - 1).max(1) as f32;
                let theta = s * twist + arm_offset;
                let r = radius * s;
                let center = Vec2::new(theta.cos(), theta.sin()) * r;

                let ds = 1.0 / (density - 1).max(1) as f32;
                let theta_ahead = (s + ds).min(1.0) * twist + arm_offset;
                let ahead = Vec2::new(theta_ahead.cos(), theta_ahead.sin()) * radius * (s + ds).min(1.0);
                let tangent = (ahead - center).normalize_or_zero();
                let normal = Vec2::new(-tangent.y, tangent.x);
                let hw = thickness * 0.5 * (0.3 + 0.7 * s);

                vertices.push(Vertex::new(center + normal * hw, s));
                vertices.push(Vertex::new(center - normal * hw, s));
            }
            strips.push(Strip { start, len: vertices.len() as u32 - start });
        }

        pipeline.draw(ctx, target, &vertices, &strips, color);
    }
}
