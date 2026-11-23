//! The GPU half of a model's surface picture and material (plan: surface
//! sources, materials M3).
//!
//! A model that declares `Model::surfaces()` receives a picture per part as
//! plain RGBA8 pixels through `Model::set_surface`, which has no device. The
//! pixels are kept (owned) and uploaded lazily in `render`, where a `Ctx` has
//! the device and queue. The texture is sRGB, so the sampled picture is linear
//! like the rest of the shader. [`SurfaceSlot`] is the one texture a part draws
//! from: it is recreated only when the picture's size changes, rewritten in
//! place otherwise, and falls back to a 1x1 texel when the part has no image.
//!
//! **The material rides the same part.** `set_surface` takes a [`SurfaceSpec`]
//! — an optional picture and an optional [`MaterialSpec`] — and a part keeps
//! both in one [`MaterialU`], the struct `shaders/material.wgsl` reads. That
//! uniform is appended to the model's own UBO, one entry per part. [`PartSet`]
//! wraps the whole thing: a model that is `n` named parts declares
//! `PartSet::<n>::new(SURFACES)` and gets `surfaces()`, `set_surface()`,
//! `sync()`, the per-part uniforms and the bind entries in about five lines.

use crate::model::{Mapping, SurfaceSpec};

/// The WGSL sampling rule, see `shaders/surface.wgsl`: `surface_sample(tex,
/// samp, obj_pos, normal, screen_uv, mapping, scale) -> vec4<f32>`.
pub const WGSL: &str = include_str!("../../shaders/surface.wgsl");

/// One part's texture, sampler and the last size uploaded.
pub struct SurfaceSlot {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    sampler: wgpu::Sampler,
    size: (u32, u32),
    fallback: bool,
}

impl SurfaceSlot {
    /// A slot holding the 1x1 fallback texel.
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> SurfaceSlot {
        let (texture, view) = texture(device, 1, 1);
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("surface"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            address_mode_w: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });
        let slot = SurfaceSlot { texture, view, sampler, size: (1, 1), fallback: true };
        slot.write(queue, &[255, 255, 255, 255], 1, 1);
        slot
    }

    /// Uploads `pixels` (RGBA8, `width * height * 4` bytes). The texture is
    /// recreated only when the size changes; otherwise the old one is rewritten.
    pub fn upload(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        pixels: &[u8],
        width: u32,
        height: u32,
    ) {
        if self.size != (width, height) {
            let (texture, view) = texture(device, width, height);
            self.texture = texture;
            self.view = view;
            self.size = (width, height);
        }
        self.write(queue, pixels, width, height);
        self.fallback = false;
    }

    /// Puts the 1x1 fallback texel back (a part whose image was removed).
    pub fn reset(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        if self.fallback {
            return;
        }
        if self.size != (1, 1) {
            let (texture, view) = texture(device, 1, 1);
            self.texture = texture;
            self.view = view;
            self.size = (1, 1);
        }
        self.write(queue, &[255, 255, 255, 255], 1, 1);
        self.fallback = true;
    }

    pub fn view(&self) -> &wgpu::TextureView {
        &self.view
    }

    /// Pixel size of the current texture (1x1 for the fallback).
    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    pub fn sampler(&self) -> &wgpu::Sampler {
        &self.sampler
    }

    /// Whether the slot holds the fallback texel rather than a picture.
    pub fn is_fallback(&self) -> bool {
        self.fallback
    }

    fn write(&self, queue: &wgpu::Queue, pixels: &[u8], width: u32, height: u32) {
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            pixels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width * 4),
                rows_per_image: Some(height),
            },
            wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        );
    }
}

fn texture(device: &wgpu::Device, width: u32, height: u32) -> (wgpu::Texture, wgpu::TextureView) {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("surface"),
        size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    (texture, view)
}

/// The material half of a part as the shader reads it (mirrors `MaterialU` in
/// `shaders/material.wgsl`). One entry per part, appended to the model's UBO:
/// `a` is `(image mix, mapping, image scale, mat_kind)`, `b` is
/// `(mat_mix, mat_scale, mat_rough, mat_metal)`, `c` is
/// `(mat_bump, mat_env, mat_tint, mat_tint_hue)`, `d` is
/// `(mat_glitch, mat_glitch_rate, mat_glitch_seed, _)`.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct MaterialU {
    pub a: [f32; 4],
    pub b: [f32; 4],
    pub c: [f32; 4],
    pub d: [f32; 4],
}

impl Default for MaterialU {
    fn default() -> MaterialU {
        MaterialU { a: [0.0, 0.0, 1.0, 0.0], b: [0.0; 4], c: [0.0; 4], d: [0.0; 4] }
    }
}

/// One named part's surfaces on a model: the pixels it was handed, the GPU slot
/// they are uploaded to, and the uniform values that go with them. A model
/// holds one per part in its `Model::surfaces()` order.
pub struct Part {
    /// RGBA8 pixels and their size, `None` for no picture.
    picture: Option<(Vec<u8>, u32, u32)>,
    /// Set by `set`, cleared once `sync` has uploaded it.
    dirty: bool,
    /// Created on the first `sync`, which is the first place with a device.
    slot: Option<SurfaceSlot>,
    /// The part's material uniform, image fields and material fields both.
    pub uniform: MaterialU,
}

impl Default for Part {
    fn default() -> Part {
        Part { picture: None, dirty: false, slot: None, uniform: MaterialU::default() }
    }
}

impl Part {
    /// Takes a part's surfaces, or drops them all with `None`. A picture whose
    /// pixel count does not match its size is dropped rather than uploaded; a
    /// material with `mix` 0 stores its kind but draws as nothing.
    pub fn set(&mut self, spec: Option<SurfaceSpec>) {
        let (image, material) = match spec {
            Some(s) => (s.image, s.material),
            None => (None, None),
        };
        match image {
            Some(img) if img.pixels.len() == (img.width * img.height * 4) as usize => {
                let mapping = match img.mapping {
                    Mapping::Object => 0.0,
                    Mapping::Projector => 1.0,
                };
                self.picture = Some((img.pixels.to_vec(), img.width, img.height));
                self.uniform.a =
                    [img.mix.clamp(0.0, 1.0), mapping, img.scale, self.uniform.a[3]];
            }
            _ => {
                self.picture = None;
                self.uniform.a = [0.0, 0.0, 1.0, self.uniform.a[3]];
            }
        }
        match material {
            Some(m) => {
                self.uniform.a[3] = m.kind;
                self.uniform.b = [m.mix, m.scale, m.rough, m.metal];
                self.uniform.c = [m.bump, m.env, m.tint, m.tint_hue];
                self.uniform.d = [m.glitch, m.glitch_rate, m.glitch_seed, 0.0];
            }
            None => {
                self.uniform.a[3] = 0.0;
                self.uniform.b = [0.0; 4];
                self.uniform.c = [0.0; 4];
                self.uniform.d = [0.0; 4];
            }
        }
        self.dirty = true;
    }

    /// Creates the slot if needed and uploads a changed picture. Only a changed
    /// picture touches the GPU.
    pub fn sync(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        let slot = self.slot.get_or_insert_with(|| SurfaceSlot::new(device, queue));
        if self.dirty {
            match &self.picture {
                Some((px, w, h)) => slot.upload(device, queue, px, *w, *h),
                None => slot.reset(device, queue),
            }
            self.dirty = false;
        }
    }

    /// The slot to bind. Call `sync` first in the same render.
    pub fn slot(&self) -> &SurfaceSlot {
        self.slot.as_ref().expect("synced before it is bound")
    }
}

/// A model's named parts, all of them, as one field: the names it declares, one
/// [`Part`] each, and the helpers a model's `impl Model` forwards to. A model
/// that is a fixed list of parts writes `surfaces: PartSet::<2>::new(SURFACES)`
/// and five one-line forwards, and the layout and bind numbering cannot drift
/// from the count.
pub struct PartSet<const N: usize> {
    names: &'static [&'static str],
    parts: [Part; N],
}

impl<const N: usize> PartSet<N> {
    /// `names` must name exactly `N` parts, in the order the shader binds them.
    pub fn new(names: &'static [&'static str]) -> PartSet<N> {
        assert_eq!(names.len(), N, "a PartSet declares one name per part slot");
        PartSet { names, parts: std::array::from_fn(|_| Part::default()) }
    }

    /// The `Model::surfaces()` answer.
    pub fn surfaces(&self) -> &'static [&'static str] {
        self.names
    }

    /// `Model::set_surface`: unknown parts are ignored, like the default.
    pub fn set(&mut self, part: &str, spec: Option<SurfaceSpec>) {
        if let Some(at) = self.names.iter().position(|p| *p == part) {
            self.parts[at].set(spec);
        }
    }

    /// Uploads every part's picture: call once per render, before binding.
    pub fn sync(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        for part in &mut self.parts {
            part.sync(device, queue);
        }
    }

    /// The material uniform of part `i`, in `surfaces()` order.
    pub fn uniform(&self, i: usize) -> MaterialU {
        self.parts[i].uniform
    }

    /// The slot of part `i`, to bind. Call `sync` first in the same render.
    pub fn slot(&self, i: usize) -> &SurfaceSlot {
        self.parts[i].slot()
    }

    /// Texture/sampler pairs in `surfaces()` order, for
    /// `Fullscreen::draw_textured`.
    pub fn bind_pairs(&self) -> Vec<(&wgpu::TextureView, &wgpu::Sampler)> {
        self.parts
            .iter()
            .map(|part| {
                let slot = part.slot();
                (slot.view(), slot.sampler())
            })
            .collect()
    }

    /// The fragment-stage layout entries for these parts.
    pub fn layout_entries(&self) -> Vec<wgpu::BindGroupLayoutEntry> {
        layout_entries(N)
    }

    /// The bind-group entries for these parts, in the same numbering.
    pub fn bind_entries(&self) -> Vec<wgpu::BindGroupEntry<'_>> {
        let slots: Vec<&SurfaceSlot> = self.parts.iter().map(|p| p.slot()).collect();
        bind_entries(&slots)
    }
}

/// Fragment-stage layout entries for `parts` pictures, numbered from binding 1
/// (binding 0 is the model's uniform): texture at `1 + 2i`, sampler at `2 + 2i`.
/// The same layout `Fullscreen::with_textures` declares.
pub fn layout_entries(parts: usize) -> Vec<wgpu::BindGroupLayoutEntry> {
    let mut entries = Vec::with_capacity(parts * 2);
    for i in 0..parts as u32 {
        entries.push(wgpu::BindGroupLayoutEntry {
            binding: 1 + 2 * i,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        });
        entries.push(wgpu::BindGroupLayoutEntry {
            binding: 2 + 2 * i,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        });
    }
    entries
}

/// Bind-group entries for the slots, in the numbering `layout_entries` uses.
pub fn bind_entries<'a>(slots: &[&'a SurfaceSlot]) -> Vec<wgpu::BindGroupEntry<'a>> {
    let mut entries = Vec::with_capacity(slots.len() * 2);
    for (i, slot) in slots.iter().enumerate() {
        let i = i as u32;
        entries.push(wgpu::BindGroupEntry {
            binding: 1 + 2 * i,
            resource: wgpu::BindingResource::TextureView(slot.view()),
        });
        entries.push(wgpu::BindGroupEntry {
            binding: 2 + 2 * i,
            resource: wgpu::BindingResource::Sampler(slot.sampler()),
        });
    }
    entries
}
