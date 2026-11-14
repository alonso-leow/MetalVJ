//! Detects whether a compiled module declares the float-uniform block every
//! `metal-render` model uses — see the crate-level docs for the shape being
//! matched (`chamber.wgsl`'s `MeshUniform` is the reference example: a
//! `var<uniform>` struct with `floats`/`floats2`/... members, each
//! `vec4<f32>`).

use naga::{AddressSpace, Module, ScalarKind, TypeInner, VectorSize};

pub struct FloatBlock {
    /// Total byte size of the uniform struct — the buffer `raster` builds
    /// must be at least this large.
    pub struct_size: u64,
    /// How many `f32`s the `floatsN` members can hold between them.
    pub float_capacity: usize,
    /// Byte offsets of each `floatsN` member, in name order
    /// (`floats`, `floats2`, `floats3`, ...) — where `raster` writes each
    /// group of 4 floats.
    pub member_offsets: Vec<u32>,
}

/// `true` for a name that is `floats`, `floats2`, `floats3`, ... — `floats`
/// alone (no digit) or `floats` followed by an integer with no leading
/// zero. Anything else (`floats_extra`, `floatsx`) does not count, so an
/// author cannot accidentally satisfy the check with an unrelated field.
fn is_floats_member_name(name: &str) -> bool {
    let Some(rest) = name.strip_prefix("floats") else { return false };
    rest.is_empty() || (rest.chars().all(|c| c.is_ascii_digit()) && !rest.starts_with('0'))
}

fn is_vec4_f32(inner: &TypeInner) -> bool {
    matches!(
        inner,
        TypeInner::Vector { size: VectorSize::Quad, scalar } if scalar.kind == ScalarKind::Float && scalar.width == 4
    )
}

/// Scans every `var<uniform>` global for a struct type with at least one
/// `floatsN: vec4<f32>` member. Returns `None` if no global variable
/// matches at all — the case `compile` turns into
/// [`crate::CompileError::NoFloatBlock`].
/// Whether the module declares an effect's two bindings: a sampled 2D
/// texture at `@group(0) @binding(1)` and a sampler at `@binding(2)`.
pub fn effect_bindings(module: &Module) -> (bool, bool) {
    let at = |binding: u32| {
        module.global_variables.iter().find_map(|(_, g)| {
            let b = g.binding.as_ref()?;
            (b.group == 0 && b.binding == binding).then(|| module.types.get_handle(g.ty).ok())?
        })
    };
    let texture = at(1).is_some_and(|t| {
        matches!(t.inner, TypeInner::Image { dim: naga::ImageDimension::D2, arrayed: false, .. })
    });
    let sampler = at(2).is_some_and(|t| matches!(t.inner, TypeInner::Sampler { comparison: false }));
    (texture, sampler)
}

pub fn find_float_block(module: &Module) -> Option<FloatBlock> {
    for (_, global) in module.global_variables.iter() {
        if global.space != AddressSpace::Uniform {
            continue;
        }
        let Some(ty) = module.types.get_handle(global.ty).ok() else { continue };
        let TypeInner::Struct { members, span } = &ty.inner else { continue };

        let mut matches: Vec<(&str, u32)> = members
            .iter()
            .filter_map(|m| {
                let name = m.name.as_deref()?;
                if !is_floats_member_name(name) {
                    return None;
                }
                let member_ty = module.types.get_handle(m.ty).ok()?;
                is_vec4_f32(&member_ty.inner).then_some((name, m.offset))
            })
            .collect();

        if matches.is_empty() {
            continue;
        }

        // Sort `floats`, `floats2`, `floats3`, ... by the numeric suffix
        // (`floats` itself sorts first) so `member_offsets[i]` lines up with
        // "the i-th group of 4 floats" the way `raster` writes them.
        matches.sort_by_key(|(name, _)| {
            name.strip_prefix("floats")
                .filter(|s| !s.is_empty())
                .and_then(|s| s.parse::<u32>().ok())
                .unwrap_or(0)
        });

        let member_offsets: Vec<u32> = matches.into_iter().map(|(_, offset)| offset).collect();
        return Some(FloatBlock {
            struct_size: *span as u64,
            float_capacity: member_offsets.len() * 4,
            member_offsets,
        });
    }
    None
}
