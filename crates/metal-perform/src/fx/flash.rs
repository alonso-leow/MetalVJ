//! `fx.flash` (the loops design notes T13): a white flash that keeps the picture
//! visible — the strobe an effect loop fires on the beat.
//!
//! Two params, each under its own config key: `opacity` (`0..1`, how far
//! toward the flash) and `diffuse` (`0..1`, the blend). The flash is never a
//! plain white fill: at `diffuse` 0 it is a **screen** of white at `opacity`
//! (`c + (1 - c) × opacity` — everything lifts toward white, darks most); at
//! 1 it is a **soft light** of white (`c → √c`, W3C soft-light with a white
//! top layer: the picture brightens but its edges and contrast survive
//! even at full opacity); in between, the two mixed. Pure per pixel, in
//! display (sRGB byte) space, the space blend modes are defined in; alpha
//! is left alone.

/// The node kind.
pub const FX_FLASH_KIND: &str = metal_graph::FLASH_EFFECT_KIND;

/// `(opacity, diffuse)` from a node's config, clamped.
pub fn flash_of(node: &metal_graph::Node) -> (f32, f32) {
    let cfg: serde_json::Value = serde_json::from_str(&node.config.to_json_str()).unwrap_or_default();
    let get = |k: &str, d: f32| cfg.get(k).and_then(serde_json::Value::as_f64).map(|v| v as f32).unwrap_or(d).clamp(0.0, 1.0);
    (get("opacity", 0.0), get("diffuse", 0.5))
}

/// Flashes `px` (RGBA8) in place.
pub fn apply(px: &mut [u8], opacity: f32, diffuse: f32) {
    if opacity <= 0.0 {
        return;
    }
    let lut: Vec<u8> = (0..256)
        .map(|i| {
            let c = i as f32 / 255.0;
            let target = 1.0 + (c.sqrt() - 1.0) * diffuse; // screen(white) = 1, soft-light(white) = √c
            ((c + (target - c) * opacity) * 255.0).round().clamp(0.0, 255.0) as u8
        })
        .collect();
    for p in px.chunks_exact_mut(4) {
        for c in &mut p[..3] {
            *c = lut[*c as usize];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_diffuse_flash_brightens_and_keeps_the_picture() {
        let frame: Vec<u8> = (0..64u32).flat_map(|i| [(i * 4) as u8, 255 - (i * 4) as u8, 40, 255]).collect();
        let mut off = frame.clone();
        apply(&mut off, 0.0, 0.5);
        assert_eq!(off, frame, "opacity 0 is the identity");
        let mut soft = frame.clone();
        apply(&mut soft, 1.0, 1.0);
        assert!(soft.chunks(4).zip(frame.chunks(4)).all(|(a, b)| a[0] >= b[0] && a[3] == 255));
        // Content stays visible: distinct inputs stay distinct at full soft flash.
        assert!(soft[0] < soft[4 * 32] && soft[4 * 32] < soft[4 * 63]);
        let mut screen = frame.clone();
        apply(&mut screen, 0.6, 0.0);
        assert!(screen.chunks(4).all(|p| p[2] >= 160), "screen lifts the darks most");
        assert!(screen[0] < screen[4 * 63], "and still keeps the ramp");
    }
}
