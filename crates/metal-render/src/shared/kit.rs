//! The float kits every wave-2 model shares, so the AI steers all of them with
//! the same names: `the effect design notes` §1.
//!
//! A model lists its own identity floats first, then appends the kits it uses
//! as separate `Block::Floats` (plan rule 2: appended, never inserted). Read
//! each back with `Cam2d::from_floats(&floats[off..off + Cam2d::N])`; the
//! offset is the sum of the widths of the blocks before it, so compute it from
//! `Block::width`, never write the number by hand.

use crate::model::{spec, spec_held, Block, FloatSpec};

/// 2D camera: zoom, pan, roll and a slow drift.
pub const CAM2D: &[FloatSpec] = &[
    spec("zoom", 0.1, 8.0, 1.0),
    spec("pan_x", -2.0, 2.0, 0.0),
    spec("pan_y", -2.0, 2.0, 0.0),
    spec("turn", -1.0, 1.0, 0.0),
    spec("drift", 0.0, 1.0, 0.0),
];
pub const CAM2D_BLOCK: Block = Block::Floats(CAM2D);

/// 3D camera. `cam_orbita` is a speed (signed, 0 = held), `cam_dolly` pushes
/// along the view axis on top of `cam_dist`, `cam_sacudida` is beat tremor.
/// `cam_fov` is structural (a swept fov reads as a zoom, not as a cut).
pub const CAM3D: &[FloatSpec] = &[
    spec("cam_yaw", -3.1416, 3.1416, 0.0),
    spec("cam_pitch", -1.5, 1.5, 0.2),
    spec("cam_roll", -3.1416, 3.1416, 0.0),
    spec("cam_distance", 0.5, 20.0, 4.0),
    spec_held("cam_fov", 0.3, 2.2, 1.0, 0.25),
    spec("cam_orbit", -2.0, 2.0, 0.0),
    spec("cam_height", -5.0, 5.0, 0.0),
    spec("cam_dolly", -5.0, 5.0, 0.0),
    spec("cam_shake", 0.0, 1.0, 0.0),
];
pub const CAM3D_BLOCK: Block = Block::Floats(CAM3D);

/// Look: what colour slots cannot say. `background_level` is the background level
/// (0 = transparent when `Ctx::fondo_alpha`), `tono` rotates every slot's hue.
pub const LOOK: &[FloatSpec] = &[
    spec("shine", 0.0, 3.0, 1.0),
    spec("contrast", 0.2, 4.0, 1.0),
    spec("opacity", 0.0, 1.0, 1.0),
    spec("background_level", 0.0, 1.0, 1.0),
    spec("tone", 0.0, 1.0, 0.0),
    spec("saturation", 0.0, 2.0, 1.0),
];
pub const LOOK_BLOCK: Block = Block::Floats(LOOK);

/// The anti-repetition kit: a domain warp, a mirror fold, a beat kick. Added
/// to every model whose picture is otherwise one state of one function.
/// `warp_freq` and `espejo` are structural.
pub const TWIST: &[FloatSpec] = &[
    spec("warp_amount", 0.0, 1.0, 0.0),
    spec_held("warp_freq", 0.5, 12.0, 3.0, 0.2),
    spec_held("mirror", 0.0, 12.0, 0.0, 0.2),
    spec("pulse", 0.0, 1.0, 0.0),
];
pub const TWIST_BLOCK: Block = Block::Floats(TWIST);

macro_rules! kit_struct {
    ($name:ident, $src:ident, [$($f:ident),*]) => {
        #[derive(Clone, Copy, Debug)]
        pub struct $name { $(pub $f: f32),* }
        impl $name {
            pub const N: usize = $src.len();
            /// `f` is exactly `N` floats, in declaration order.
            pub fn from_floats(f: &[f32]) -> Self {
                debug_assert_eq!(f.len(), Self::N);
                let mut it = f.iter().copied();
                $name { $($f: it.next().unwrap_or(0.0)),* }
            }
        }
    };
}

kit_struct!(Cam2d, CAM2D, [zoom, pan_x, pan_y, turn, drift]);
kit_struct!(Cam3d, CAM3D, [yaw, pitch, roll, distance, fov, orbit, alt, dolly, shake]);
kit_struct!(Look, LOOK, [shine, contrast, opacity, background_level, tone, saturation]);
kit_struct!(Twist, TWIST, [warp_amount, warp_freq, mirror, pulse]);

/// Offset of a block inside a schema's float row: the sum of the widths of
/// the blocks before `index`.
pub fn offset_of(blocks: &[Block], index: usize) -> usize {
    blocks[..index].iter().map(|b| b.width()).sum()
}

/// WGSL shared by the kit users: hue rotation, 2D camera transform, mirror
/// fold and domain warp. Prepend to a model's WGSL with `format!`/`concat!`.
pub const WGSL: &str = r#"
fn kit_hue(c: vec3<f32>, h: f32) -> vec3<f32> {
    let k = vec3<f32>(0.57735);
    let a = h * 6.28318;
    return c * cos(a) + cross(k, c) * sin(a) + k * dot(k, c) * (1.0 - cos(a));
}
fn kit_look(c: vec3<f32>, shine: f32, contraste: f32, tono: f32, sat: f32) -> vec3<f32> {
    var o = kit_hue(c, tono);
    let l = dot(o, vec3<f32>(0.299, 0.587, 0.114));
    o = mix(vec3<f32>(l), o, sat);
    o = (o - 0.5) * contraste + 0.5;
    return max(o * shine, vec3<f32>(0.0));
}
fn kit_cam2d(uv: vec2<f32>, aspect: f32, zoom: f32, pan: vec2<f32>, giro: f32, deriva: f32, t: f32) -> vec2<f32> {
    var p = (uv - 0.5) * vec2<f32>(aspect, 1.0);
    p = p / max(zoom, 0.01);
    let a = giro * 3.14159 + deriva * 0.2 * t;
    let c = cos(a); let s = sin(a);
    p = vec2<f32>(c * p.x - s * p.y, s * p.x + c * p.y);
    return p + pan;
}
fn kit_mirror(p: vec2<f32>, n: f32) -> vec2<f32> {
    if (n < 1.5) { return p; }
    let seg = 6.28318 / floor(n);
    let r = length(p);
    var a = atan2(p.y, p.x);
    a = abs((a + 3.14159) % seg - seg * 0.5);
    return vec2<f32>(cos(a), sin(a)) * r;
}
fn kit_warp(p: vec2<f32>, amt: f32, freq: f32, t: f32) -> vec2<f32> {
    if (amt <= 0.0) { return p; }
    return p + amt * 0.25 * vec2<f32>(sin(p.y * freq + t * 0.7), cos(p.x * freq - t * 0.6));
}
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kits_round_trip_in_declaration_order() {
        let d: Vec<f32> = CAM3D.iter().map(|s| s.default).collect();
        let c = Cam3d::from_floats(&d);
        assert_eq!(c.distance, 4.0);
        assert_eq!(Cam2d::N, CAM2D.len());
    }

    #[test]
    fn no_float_name_repeats_across_the_kits() {
        let mut names: Vec<&str> =
            [CAM2D, CAM3D, LOOK, TWIST].iter().flat_map(|k| k.iter().map(|s| s.name)).collect();
        names.sort();
        let n = names.len();
        names.dedup();
        assert_eq!(n, names.len());
    }
}
