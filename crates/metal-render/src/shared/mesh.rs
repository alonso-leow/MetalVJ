//! What the four 3D models (Chamber, Membrane, Oracle, POV) share: the
//! vertex layout, the depth buffer, and the pipeline-descriptor boilerplate
//! around them. Each model still writes its own WGSL and generates its own
//! geometry — those are genuinely different per model — but the plumbing
//! around a lit, depth-tested triangle mesh is one thing repeated four
//! times, so it lives here once.

pub const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vertex {
    pub pos: [f32; 3],
    pub normal: [f32; 3],
    pub t: f32,
}

impl Vertex {
    pub fn new(pos: glam::Vec3, normal: glam::Vec3, t: f32) -> Vertex {
        Vertex { pos: pos.into(), normal: normal.into(), t }
    }
}

pub const VERTEX_LAYOUT: wgpu::VertexBufferLayout<'static> = wgpu::VertexBufferLayout {
    array_stride: std::mem::size_of::<Vertex>() as wgpu::BufferAddress,
    step_mode: wgpu::VertexStepMode::Vertex,
    attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32],
};

/// A depth view, rebuilt only when the requested size changes.
#[derive(Default)]
pub struct DepthCache {
    view: Option<wgpu::TextureView>,
    size: (u32, u32),
}

impl DepthCache {
    pub fn get(&mut self, device: &wgpu::Device, width: u32, height: u32) -> &wgpu::TextureView {
        if self.view.is_none() || self.size != (width, height) {
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("mesh depth"),
                size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: DEPTH_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            });
            self.view = Some(texture.create_view(&wgpu::TextureViewDescriptor::default()));
            self.size = (width, height);
        }
        self.view.as_ref().expect("just set")
    }
}

/// Builds a standard lit-mesh pipeline: one colour target with alpha blending,
/// depth-tested against `DEPTH_FORMAT`, no culling (every model here is
/// thin/open geometry — hair strands, a grid, eyelids — where backface culling
/// would just as often hide the wrong side as the right one).
///
/// `layout` is the vertex layout the model feeds, [`VERTEX_LAYOUT`] for a
/// `Vertex` mesh and the weave's own for a tagged/grid one; a model that draws
/// no geometry at all passes `None`.
pub fn build_pipeline(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    shader: &wgpu::ShaderModule,
    bind_group_layout: &wgpu::BindGroupLayout,
    label: &str,
    topology: wgpu::PrimitiveTopology,
    layout: Option<wgpu::VertexBufferLayout<'static>>,
) -> wgpu::RenderPipeline {
    build_pipeline_ex(device, format, shader, bind_group_layout, label, topology, layout, "fs_main", true)
}

/// [`build_pipeline`] with an explicit fragment entry point and the depth-write
/// toggle, for a model that draws a second, translucent list of the same
/// geometry beside its opening pipeline (the materials' filled layer).
pub fn build_pipeline_ex(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    shader: &wgpu::ShaderModule,
    bind_group_layout: &wgpu::BindGroupLayout,
    label: &str,
    topology: wgpu::PrimitiveTopology,
    layout: Option<wgpu::VertexBufferLayout<'static>>,
    fragment_entry: &str,
    depth_write: bool,
) -> wgpu::RenderPipeline {
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some(label),
        bind_group_layouts: &[Some(bind_group_layout)],
        immediate_size: 0,
    });

    let single;
    let buffers: &[Option<wgpu::VertexBufferLayout>] = match layout {
        Some(l) => {
            single = [Some(l)];
            &single
        }
        None => &[],
    };

    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_main"),
            buffers,
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some(fragment_entry),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: Default::default(),
        }),
        primitive: wgpu::PrimitiveState { topology, cull_mode: None, ..Default::default() },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: Some(depth_write),
            depth_compare: Some(wgpu::CompareFunction::Less),
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    })
}

/// A tapered four-sided prism from `a` to `b`, `r0` thick at `a` and `r1` at
/// `b`, capped at the far end. `wide` is the direction the flat of it faces and
/// `flat` how much thinner it is across that axis than along it; `t` tags every
/// vertex so a colour slot can tell one part from another.
///
/// The rod half of a geometry vocabulary for rig-driven bodies — `node`,
/// below, is the other half. They live here because any two such models must
/// agree — two copies of "a bone" is two bodies that taper differently, and
/// the difference only shows up beside each other.
#[allow(clippy::too_many_arguments)]
pub fn prism(
    out: &mut Vec<Vertex>,
    a: glam::Vec3,
    b: glam::Vec3,
    r0: f32,
    r1: f32,
    wide: glam::Vec3,
    flat: f32,
    t: f32,
) {
    use glam::Vec3;

    let axis = (b - a).normalize_or_zero();
    if axis.length_squared() < 1e-8 {
        return;
    }
    let mut u = wide - axis * wide.dot(axis);
    if u.length_squared() < 1e-8 {
        u = axis.cross(Vec3::Y);
    }
    let u = u.normalize_or_zero();
    let v = axis.cross(u).normalize_or_zero();

    let (fu, fv) = (1.0f32, flat);
    let ring = |c: Vec3, r: f32| {
        [
            c + u * (r * fu) + v * (r * fv),
            c - u * (r * fu) + v * (r * fv),
            c - u * (r * fu) - v * (r * fv),
            c + u * (r * fu) - v * (r * fv),
        ]
    };
    let lo = ring(a, r0);
    let hi = ring(b, r1);
    let centre = (a + b) * 0.5;
    for k in 0..4 {
        let k2 = (k + 1) % 4;
        let n = ((lo[k] + lo[k2] + hi[k] + hi[k2]) * 0.25 - centre).normalize_or_zero();
        for p in [lo[k], hi[k], lo[k2], lo[k2], hi[k], hi[k2]] {
            out.push(Vertex::new(p, n, t));
        }
    }
    // The cap, so a tip is a tip and not a hole.
    for tri in [[hi[0], hi[1], hi[2]], [hi[0], hi[2], hi[3]]] {
        for p in tri {
            out.push(Vertex::new(p, axis, t));
        }
    }
}

/// A ball at `center`, `radius` across — the joint marker for the two models
/// that draw a body off `metal-rig`. `rod` is what runs between two points; this
/// is what sits at the point, and drawing both rather than only the connector
/// is the difference between a skeleton and a limb.
///
/// Radial per-vertex normals, so it shades round rather than faceted.
///
/// **Tessellated for the near view, not the far one.** 8×16 is far more than
/// a figure at stage distance needs and exactly what POV needs: a wrist there
/// fills a quarter of the frame, and at 3×6 it read as a visible hexagon —
/// the one thing this rework exists to stop looking like. The cost is bounded
/// and small (a few hundred balls a frame at 256 triangles each, on hardware
/// that draws a Turing reaction), so the near view sets the number.
pub fn node(out: &mut Vec<Vertex>, center: glam::Vec3, radius: f32, t: f32) {
    use glam::Vec3;
    use std::f32::consts::{PI, TAU};

    const RINGS: i32 = 8;
    const SEGMENTS: i32 = 16;

    let dir = |ring: i32, seg: i32| -> Vec3 {
        let phi = (ring as f32 / RINGS as f32) * PI;
        let theta = (seg as f32 / SEGMENTS as f32) * TAU;
        Vec3::new(phi.sin() * theta.cos(), phi.cos(), phi.sin() * theta.sin())
    };
    for r in 0..RINGS {
        for s in 0..SEGMENTS {
            for (rr, ss) in [(r, s), (r + 1, s), (r, s + 1), (r, s + 1), (r + 1, s), (r + 1, s + 1)] {
                let d = dir(rr, ss);
                out.push(Vertex::new(center + d * radius, d, t));
            }
        }
    }
}

/// A round tapered rod from `a` to `b`, `r0` thick at `a` and `r1` at `b` —
/// what runs between two `node` balls.
///
/// `prism` would do the job and used to, but a four-sided box reads as a
/// square stick the moment it is more than a few pixels across, which is most
/// of POV. Radial per-vertex normals for the same reason `node` has them.
///
/// **Uncapped on purpose.** Every rod in both models runs between two balls
/// that are wider than it is (`ROD_OF_NODE` is well under 1), so a cap is
/// geometry that is always inside a sphere — invisible, and a normal seam
/// where the shading would otherwise be continuous.
pub fn rod(out: &mut Vec<Vertex>, a: glam::Vec3, b: glam::Vec3, r0: f32, r1: f32, t: f32) {
    use glam::Vec3;
    use std::f32::consts::TAU;

    const SIDES: i32 = 12;

    let axis = (b - a).normalize_or_zero();
    if axis.length_squared() < 1e-8 {
        return;
    }
    let mut u = axis.cross(Vec3::Y);
    if u.length_squared() < 1e-8 {
        u = axis.cross(Vec3::X);
    }
    let u = u.normalize_or_zero();
    let v = axis.cross(u).normalize_or_zero();

    let dir = |k: i32| {
        let theta = (k as f32 / SIDES as f32) * TAU;
        u * theta.cos() + v * theta.sin()
    };
    for k in 0..SIDES {
        let (d0, d1) = (dir(k), dir(k + 1));
        for (p, n) in [
            (a + d0 * r0, d0),
            (b + d0 * r1, d0),
            (a + d1 * r0, d1),
            (a + d1 * r0, d1),
            (b + d0 * r1, d0),
            (b + d1 * r1, d1),
        ] {
            out.push(Vertex::new(p, n, t));
        }
    }
}

/// `CameraGpu::view_proj` as a raw column-major array, the form every 3D
/// model's uniform carries it in.
pub fn view_proj_cols(pos: glam::Vec3, target: glam::Vec3, fov_degrees: f32, aspect: f32) -> [[f32; 4]; 4] {
    let view = glam::Mat4::look_at_rh(pos, target, glam::Vec3::Y);
    let proj = glam::Mat4::perspective_rh(
        fov_degrees.to_radians().clamp(0.05, std::f32::consts::PI - 0.05),
        aspect,
        0.05,
        100.0,
    );
    (proj * view).to_cols_array_2d()
}
