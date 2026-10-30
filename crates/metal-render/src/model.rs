//! The contract every renderer shares: a schema describing the floats it
//! accepts, and a `render` that turns one row of them into a frame. A model
//! never knows whether the row came from a slider or the digest — that is
//! the entire point of the suite.
//!
//! **The schema half of this module now lives in `metal-schema`** and is
//! re-exported below, unchanged, so every `metal_render::model::Schema` in the
//! workspace still resolves. The split is not cosmetic: `Schema`, `Block` and
//! `FloatSpec` are plain data that several consumers want, and leaving them
//! here made wanting them mean linking wgpu, winit and egui. What stayed is
//! everything that genuinely needs a GPU — `Ctx`, `Built` and [`Model`] — and
//! that is exactly the line between "what a model accepts" and "how it draws".
//!
//! `metal-schema` has the whole argument, including the consumer that forced it.

pub use metal_schema::{
    camera, color, color_at, background, read_post, read_signal, solid4, spec, spec_held, spec_rig,
    Block,
    Capabilities, Capability, FloatSpec, Schema, Signal, BLACK4, EFFECTS_COLOR_BLOCK,
    EFFECTS_FLOATS_BLOCK, EFFECTS_WIDTH, SIGNAL_FLOATS_BLOCK, SIGNAL_WIDTH, WHITE4,
};

// ---------------------------------------------------------------------------
// H4: the off-axis camera
// ---------------------------------------------------------------------------

/// The generalized-perspective ("off-axis") projection: the eye moves, the
/// screen plane stays fixed, so a viewer's own movement adds parallax instead
/// of sliding the whole picture. The screen is the rectangle of half-extents
/// `half_w`/`half_h` centred at `screen`, oriented by the line from `rest` (the
/// eye at zero track) to `screen`.
///
/// `rest` fixes the screen's orientation: it is the resting eye the model was
/// framed against. A camera whose eye is at `rest` gets the ordinary symmetric
/// frustum back, which is what keeps a zero track byte-identical to before.
pub fn off_axis_with(
    eye: [f32; 3],
    screen: [f32; 3],
    rest: [f32; 3],
    half_w: f32,
    half_h: f32,
) -> [[f32; 4]; 4] {
    use glam::{Mat4, Vec3, Vec4};
    let eye = Vec3::from(eye);
    let screen = Vec3::from(screen);
    let rest = Vec3::from(rest);
    let forward = (screen - rest).normalize_or_zero();
    let right = forward.cross(Vec3::Y).normalize_or_zero();
    let up = right.cross(forward).normalize_or_zero();
    let (near, far) = (0.05f32, 100.0f32);
    let view = Mat4::look_at_rh(eye, screen, Vec3::Y);
    // The four screen corners in world, projected into the eye's frame; the
    // frustum's bounds are the tightest box around them at the near plane.
    let mut l = f32::MAX;
    let mut r = f32::MIN;
    let mut b = f32::MAX;
    let mut t = f32::MIN;
    for corner in [
        screen + right * half_w + up * half_h,
        screen - right * half_w + up * half_h,
        screen + right * half_w - up * half_h,
        screen - right * half_w - up * half_h,
    ] {
        let v = view.transform_point3(corner);
        let d = -v.z;
        if d <= 1e-4 {
            continue;
        }
        let x = v.x * near / d;
        let y = v.y * near / d;
        l = l.min(x);
        r = r.max(x);
        b = b.min(y);
        t = t.max(y);
    }
    // The same off-axis form glam's `perspective_rh` uses (RH, `[0,1]` depth),
    // with the skew terms kept.
    let w = 2.0 * near / (r - l).max(1e-6);
    let h = 2.0 * near / (t - b).max(1e-6);
    let depth = far / (near - far);
    let proj = Mat4::from_cols(
        Vec4::new(w, 0.0, 0.0, 0.0),
        Vec4::new(0.0, h, 0.0, 0.0),
        Vec4::new((r + l) / (r - l).max(1e-6), (t + b) / (t - b).max(1e-6), depth, -1.0),
        Vec4::new(0.0, 0.0, depth * near, 0.0),
    );
    (proj * view).to_cols_array_2d()
}

/// [`off_axis_with`]'s plain case: a unit screen one metre in front of a rest
/// eye at `screen + (0, 0, 1)` — the H4 signature, and what a caller with no
/// screen size to hand uses.
pub fn off_axis(eye: [f32; 3], screen: [f32; 3]) -> [[f32; 4]; 4] {
    off_axis_with(eye, screen, [screen[0], screen[1], screen[2] + 1.0], 1.0, 1.0)
}

/// The three reserved `cam_x/cam_y/cam_z` floats at the end of a model's own
/// blocks, just before the shared signal/effects tail (plan rule 2). `[0,0,0]`
/// for a row too short to carry them (an older build), which is "no track".
pub fn camera_track_offset(floats: &[f32]) -> [f32; 3] {
    match floats.len().checked_sub(metal_schema::SIGNAL_WIDTH + metal_schema::EFFECTS_WIDTH + 3) {
        Some(at) => [floats[at], floats[at + 1], floats[at + 2]],
        None => [0.0; 3],
    }
}

/// The view-projection a 3D model uses: `view_proj_cols`' symmetric frustum
/// when the track is zero, and [`off_axis_with`] when it is not. `rest_eye` is
/// the eye the model framed itself against (before the track is added).
pub fn tracked_view_proj(
    rest_eye: glam::Vec3,
    target: glam::Vec3,
    fov_degrees: f32,
    aspect: f32,
    track: [f32; 3],
) -> [[f32; 4]; 4] {
    if track == [0.0, 0.0, 0.0] {
        let view = glam::Mat4::look_at_rh(rest_eye, target, glam::Vec3::Y);
        let proj = glam::Mat4::perspective_rh(
            fov_degrees.to_radians().clamp(0.05, std::f32::consts::PI - 0.05),
            aspect,
            0.05,
            100.0,
        );
        return (proj * view).to_cols_array_2d();
    }
    let eye = rest_eye + glam::Vec3::from(track);
    let d = (target - rest_eye).length().max(1e-3);
    let half_h = d * (fov_degrees.to_radians() * 0.5).tan();
    let half_w = half_h * aspect.max(1e-3);
    off_axis_with(eye.into(), target.into(), rest_eye.into(), half_w, half_h)
}

/// What a model needs to draw one frame: the device to build resources with,
/// the queue to submit to, and how much time has passed.
pub struct Ctx<'a> {
    pub device: &'a wgpu::Device,
    pub queue: &'a wgpu::Queue,
    pub time: f32,
    pub width: u32,
    pub height: u32,
    /// Whether the current source's `fondo` slot should render as alpha
    /// (transparent) instead of its own colour, for the twelve models whose
    /// [`Model::backdrop`] is `true` — see `the design notes Part IV` §17.3. Ignored
    /// by every other model.
    pub fondo_alpha: bool,
    /// How much of the authored particle/step/iteration counts this frame can
    /// afford: the tile's pixel count over 1920x1080, clamped to 0.25..=1.0 (see
    /// `headless::detail_factor`). `1.0` at 1080p and above, so big outputs are
    /// unchanged. Read by scaffold shaders through `p_detail()`.
    pub detail: f32,
}

impl Ctx<'_> {
    pub fn aspect(&self) -> f32 {
        self.width as f32 / self.height.max(1) as f32
    }
}

/// The GPU resources a model builds on its first frame and keeps afterwards.
///
/// Keyed on the target format, which is the whole reason this is not a plain
/// `Option`: a pipeline is compiled against one colour format, and the suite
/// now has more than one consumer — the window renders at `Rgba16Float`, the
/// preview server at `Rgba8UnormSrgb`. A model that cached on first use alone
/// would hand the second consumer a pipeline built for the first one's format
/// and fail validation at draw time, far from the cause.
pub struct Built<T> {
    inner: Option<(wgpu::TextureFormat, T)>,
}

impl<T> Default for Built<T> {
    fn default() -> Self {
        Built { inner: None }
    }
}

impl<T> Built<T> {
    pub fn get(&mut self, format: wgpu::TextureFormat, build: impl FnOnce() -> T) -> &mut T {
        if self.inner.as_ref().map(|(f, _)| *f) != Some(format) {
            self.inner = Some((format, build()));
        }
        &mut self.inner.as_mut().expect("just built above").1
    }
}

/// `Send` on every native target, nothing on the browser. wgpu's web backend
/// hands out handles that are not `Send` (they hold `JsValue`s), and the page
/// has one thread, so nothing there needs the bound. Native keeps it.
#[cfg(not(target_arch = "wasm32"))]
pub trait MaybeSend: Send {}
#[cfg(not(target_arch = "wasm32"))]
impl<T: Send> MaybeSend for T {}
#[cfg(target_arch = "wasm32")]
pub trait MaybeSend {}
#[cfg(target_arch = "wasm32")]
impl<T> MaybeSend for T {}

/// `Send` is a supertrait, not a real constraint: every implementor is wgpu
/// pipelines, buffers and textures, all `Send + Sync` on every backend wgpu
/// supports — this was a missing bound, not a design decision that needed
/// weighing. It exists so `Box<dyn Model>` (and therefore
/// `Vec<Box<dyn Model>>` inside [`crate::headless::Renderer`]) is `Send`,
/// which is what lets `metal-dash` (`S.U7b`) hold its renderers behind an
/// `Arc<Mutex<…>>` shared between the winit thread and the HTTP listener
/// thread — see that batch's note on `struct Shared` in `metal-dash/src/main.rs`
/// for the full story of why this was deferred out of `S.U6`.
pub trait Model: MaybeSend {
    /// The stable key this model is known by outside this crate: the id in
    /// `data/schema.json`, the folder an authored example lands in
    /// (`data/animations/<id>/<song>.json`), and what a `/live` push
    /// names. Unlike `name()` it is ASCII and never changes for cosmetic
    /// reasons — renaming it orphans every example authored against it.
    fn id(&self) -> &'static str;
    /// What a human reads. Free to change.
    fn name(&self) -> &'static str;
    fn schema(&self) -> &'static Schema;

    /// Consumes exactly `schema().width()` floats and draws into `target`,
    /// which is sized `ctx.width x ctx.height` and uses `format`.
    fn render(
        &mut self,
        ctx: &Ctx,
        floats: &[f32],
        target: &wgpu::TextureView,
        format: wgpu::TextureFormat,
    );

    /// The fraction of the output's linear size this model renders at, before
    /// the post pass upscales it back to full size. `1.0` is the authored look;
    /// the heavy fullscreen models say `0.6`, which is roughly 2.8x fewer
    /// fragments. The renderer multiplies this by its global quality.
    fn render_scale(&self) -> f32 {
        1.0
    }

    /// Whether this model draws over **source material** rather than generating
    /// its own picture — a *transmission*, in `docs/TRANSMISSIONS.md`'s word.
    ///
    /// It is on the trait and in `data/schema.json` rather than being inferred
    /// from [`Model::reel`], because the app has to know it on the build with
    /// no GPU: a transmission is the one kind of model that **cannot be
    /// selected until the operator has pointed it at a folder**, and the
    /// surface that says so is drawn by `metal-dash` whether or not there is an
    /// adapter in the machine.
    fn footage(&self) -> bool {
        false
    }

    /// Whether this model draws its own background via the shared
    /// `metal_render::shared::Backdrop` pass, rather than covering the frame with
    /// its own field (Fractals, Kaleido, Turing, Domain, Vaporgrid's sky,
    /// Oracle's skin, …). Exactly the twelve models where marking `fondo` as
    /// alpha does anything — see `the design notes Part IV` §17.3. The API and the UI
    /// both call this rather than special-case model ids, so the list cannot
    /// drift from what is actually wired.
    fn backdrop(&self) -> bool {
        false
    }

    /// Whether this model dispatches compute shaders. A device without them
    /// (WebGL2) cannot build its pipelines, so the painter draws such a model
    /// on the CPU stand-in instead of asking the device for it. A model with a
    /// fragment ping-pong fallback for such a device does not say so; only the
    /// ones still compute-only do.
    fn needs_compute(&self) -> bool {
        false
    }

    /// Whether this model declares a 3D viewpoint a `cam.track` node may
    /// drive — `the design notes` §13.2. `true` means this model's
    /// [`Schema`] carries three reserved floats, `cam_x`/`cam_y`/`cam_z`,
    /// appended after everything else it declares, which [`Model::render`]
    /// is expected to read as a view-offset on top of its own resting
    /// camera. `false` — the default — means those floats do not exist for
    /// this model at all, which is why `cam.track` reads this rather than a
    /// checkbox: a model that never opts in is never offered as a
    /// destination.
    ///
    /// **Audited per model, not assumed.** §13.2 is explicit that most of
    /// the suite has no 3D viewpoint to move — a flat field like Kaleido or
    /// Fractals has nothing a "camera" would mean — so this defaults to
    /// `false` and gets turned on one model at a time, measured, rather than
    /// declared across all thirty-two and found to be ignored by half of
    /// them. `Turing` is the pilot; see its `impl Model` for the one
    /// override that exists today.
    fn camera_track(&self) -> bool {
        false
    }

    /// The footage side of a transmission, for the app to attach a library and
    /// hand it what the Curator decided. `None` for the thirty models that
    /// generate.
    ///
    /// A second method rather than parameters on [`Model::render`], for the
    /// same reason `digest::Source::beats` is a second method beside `poll`:
    /// a cue changes at most every `HOLD_SECONDS` and a frame is drawn sixty
    /// times a second, so they are not the same clock and threading one through
    /// the other's signature would make every generative model carry an
    /// argument it has no use for.
    fn reel(&mut self) -> Option<&mut dyn Reel> {
        None
    }

    /// Hands a model the WGSL it should draw next, keyed by the shader's id.
    /// Only `forge` draws runtime WGSL; every compiled model refuses, which
    /// is the default. The source is validated (`metal_forge::check`) here, so
    /// an `Err` carries naga's line and column.
    fn set_source(&mut self, key: &str, _wgsl: &str) -> Result<(), String> {
        Err(format!("{} draws its own compiled shader, not {key}", self.id()))
    }

    /// Whether this model is a **3D model** — one that has surfaces a material
    /// can be laid on, as opposed to a 2D field or a volumetric (a density or
    /// emission, not a surface). Only the surfaces default reads it: see
    /// [`Model::surfaces`]. `false` for the flat field models that make up most
    /// of the suite.
    fn is_3d(&self) -> bool {
        false
    }

    /// The named parts a surface picture — or a material — can be laid on
    /// (plan: surface sources), e.g. `["skin", "eyes"]`. Emitted into
    /// `data/schema.json` as `surfaces`, and each becomes a `tex.<part>` input
    /// on the model's generated node.
    ///
    /// **A 3D model defaults to one part, `["body"]`**, so every model that
    /// declares [`Model::is_3d`] can receive a material without naming its
    /// parts; a model with natural parts overrides this with the real names and
    /// keeps them. A 2D or volumetric model keeps the empty default, which is
    /// what a later `mat.*` lint warns on: a material on a model with no
    /// surfaces is a no-op.
    fn surfaces(&self) -> &'static [&'static str] {
        if self.is_3d() {
            &["body"]
        } else {
            &[]
        }
    }

    /// Hands a part its surfaces, or takes them away with `None`. Has no
    /// device, so a model keeps the pixels and uploads them in its next
    /// `render`. Default: a no-op, for a model with no surfaces.
    fn set_surface(&mut self, _part: &str, _spec: Option<SurfaceSpec>) {}

    /// Whether this row is a **tombstone**: a model that was retired but whose
    /// table position must stay, so an operator's positional `/select` and
    /// every routed class after it do not move (see `registry.rs`'s module
    /// doc). A tombstone keeps its `id` — so a saved reference still resolves
    /// — renders nothing, and declares no capabilities. Every derived surface
    /// (the `data/schema.json` manifest, `tools/song-gate/models.json`, the
    /// graph catalog, the demo previews) drops it rather than offering a row
    /// that draws a black frame.
    fn retired(&self) -> bool {
        false
    }

    /// What this renderer can do and needs — `docs/plan-architecture.md` D4/LAY.2.
    ///
    /// **Derived from the methods above, not a parallel declaration.** A model
    /// that answers `footage() == true` reports [`Capability::NeedsFootage`]
    /// without anyone having to remember to say so a second time, which is
    /// exactly the drift the old three booleans invited. Every model is
    /// assumed `needs_gpu` because every `render` submits to a `wgpu::Queue`;
    /// an override is where a future CPU-only model would subtract it.
    fn capabilities(&self) -> Capabilities {
        let mut c = Capabilities::empty().with(Capability::NeedsGpu);
        if self.footage() {
            c = c.with(Capability::NeedsFootage);
        }
        if self.backdrop() {
            c = c.with(Capability::Backdrop);
        }
        if self.camera_track() {
            c = c.with(Capability::CameraTrack);
        }
        c
    }
}

/// How a surface picture is laid on a part (plan: surface sources).
/// `Object` samples in object space, triplanar, so the picture follows the
/// mesh; `Projector` samples by screen position, like a projector on the part.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mapping {
    Object,
    Projector,
}

/// One part's picture, as a consumer hands it to [`Model::set_surface`]:
/// `pixels` is RGBA8, `width * height * 4` bytes. `mix` is 0..1 (0 draws the
/// part as it was, 1 replaces its albedo); `scale` is the picture's size on the
/// part (1 = once across it).
#[derive(Clone, Copy, Debug)]
pub struct SurfaceImage<'a> {
    pub pixels: &'a [u8],
    pub width: u32,
    pub height: u32,
    pub mix: f32,
    pub mapping: Mapping,
    pub scale: f32,
}

/// The two things a part can carry, in one argument: an optional picture and
/// an optional material. `None`/`None` is "nothing", which is what every part
/// is reset to before each render so a shared model instance never leaks one
/// node's surface into the next. The plan's widening of
/// `set_surface(part, Option<SurfaceImage>)` happened once, here.
#[derive(Clone, Copy, Debug, Default)]
pub struct SurfaceSpec<'a> {
    pub image: Option<SurfaceImage<'a>>,
    pub material: Option<MaterialSpec>,
}

impl<'a> SurfaceSpec<'a> {
    /// A spec that is only a picture, the common case.
    pub fn image(
        pixels: &'a [u8],
        width: u32,
        height: u32,
        mix: f32,
        mapping: Mapping,
        scale: f32,
    ) -> SurfaceSpec<'a> {
        SurfaceSpec {
            image: Some(SurfaceImage { pixels, width, height, mix, mapping, scale }),
            material: None,
        }
    }

    /// A spec that is only a material.
    pub fn material(material: MaterialSpec) -> SurfaceSpec<'a> {
        SurfaceSpec { image: None, material: Some(material) }
    }
}

/// One part's material, the row's `material` group resolved for that part
/// (`metal_schema::MATERIAL_FLOATS` order). `kind` is the `mat_kind` enum
/// (`0..=6` = matte, concrete, wood, leather, metal, glass, matrix); `mix` 0
/// is off; the rest are the operator's seasoning over the kind's own look. See
/// `shaders/material.wgsl` for how each is spent.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MaterialSpec {
    pub kind: f32,
    pub mix: f32,
    pub scale: f32,
    pub rough: f32,
    pub metal: f32,
    pub bump: f32,
    pub env: f32,
    pub tint: f32,
    pub tint_hue: f32,
    pub glitch: f32,
    pub glitch_rate: f32,
    pub glitch_seed: f32,
}

/// The footage half of a transmission — re-exported from `metal-reel`, where it
/// lives because `metal-dash`'s build with no GPU has to name it and does not have
/// this crate.
pub use metal_reel::Reel;

#[cfg(test)]
mod surface_default_tests {
    use super::{Ctx, Model, Schema};

    struct Stub(bool);

    impl Model for Stub {
        fn id(&self) -> &'static str {
            "stub"
        }
        fn name(&self) -> &'static str {
            "Stub"
        }
        fn schema(&self) -> &'static Schema {
            static SCHEMA: Schema = Schema { blocks: &[] };
            &SCHEMA
        }
        fn is_3d(&self) -> bool {
            self.0
        }
        fn render(&mut self, _: &Ctx, _: &[f32], _: &wgpu::TextureView, _: wgpu::TextureFormat) {
            unimplemented!("the surfaces default does not draw")
        }
    }

    /// A 3D model with no parts of its own takes the one part `body`; a 2D or
    /// volumetric model takes none, and a model that names its parts keeps them.
    #[test]
    fn a_3d_model_defaults_to_one_body_part_and_a_flat_one_to_none() {
        assert_eq!(Stub(true).surfaces(), &["body"]);
        assert!(Stub(false).surfaces().is_empty());

        struct Named;
        impl Model for Named {
            fn id(&self) -> &'static str {
                "named"
            }
            fn name(&self) -> &'static str {
                "Named"
            }
            fn schema(&self) -> &'static Schema {
                static SCHEMA: Schema = Schema { blocks: &[] };
                &SCHEMA
            }
            fn is_3d(&self) -> bool {
                true
            }
            fn surfaces(&self) -> &'static [&'static str] {
                &["skin", "eyes"]
            }
            fn render(&mut self, _: &Ctx, _: &[f32], _: &wgpu::TextureView, _: wgpu::TextureFormat) {
                unimplemented!()
            }
        }
        assert_eq!(Named.surfaces(), &["skin", "eyes"]);
    }
}

#[cfg(test)]
mod capability_tests {
    use super::{Capabilities, Capability};
    use crate::registry::all_models;

    #[test]
    fn every_model_reports_the_capabilities_its_methods_promise() {
        let models = all_models();
        assert!(!models.is_empty(), "the registry is empty; this gate checks nothing");
        for m in &models {
            if m.retired() {
                // A tombstone draws nothing and declares nothing; the gate is
                // about live models — see `registry.rs`.
                assert!(m.capabilities().is_empty(), "{} is retired and must claim nothing", m.id());
                continue;
            }
            let c = m.capabilities();
            assert!(c.has(Capability::NeedsGpu), "{} draws on the GPU and must say so", m.id());
            assert_eq!(
                c.has(Capability::NeedsFootage),
                m.footage(),
                "{} footage() and needs_footage disagree",
                m.id()
            );
            assert_eq!(
                c.has(Capability::Backdrop),
                m.backdrop(),
                "{} backdrop() and backdrop capability disagree",
                m.id()
            );
            assert_eq!(
                c.has(Capability::CameraTrack),
                m.camera_track(),
                "{} camera_track() and camera_track capability disagree",
                m.id()
            );
        }
    }

    #[test]
    fn an_empty_capability_set_has_nothing_and_unions() {
        assert!(Capabilities::empty().is_empty());
        let c = Capabilities::empty().with(Capability::HasAux).with(Capability::Stateful);
        assert!(c.has(Capability::HasAux) && c.has(Capability::Stateful));
        assert!(!c.has(Capability::NeedsGpu));
        assert_eq!(c.names(), vec!["stateful", "has_aux"]);
    }
}

#[cfg(test)]
mod camera_tests {
    use super::{off_axis, off_axis_with, tracked_view_proj};
    use glam::{Vec3, Vec4};

    fn project(m: [[f32; 4]; 4], p: Vec3) -> Vec4 {
        let m = glam::Mat4::from_cols_array_2d(&m);
        let v = m * p.extend(1.0);
        v / v.w
    }

    #[test]
    fn a_resting_eye_gets_the_symmetric_frustum_back() {
        let rest = [0.0, 0.0, 2.0];
        let screen = [0.0, 0.0, 0.0];
        let fov = 60.0f32;
        let aspect = 16.0 / 9.0;
        let d = 2.0;
        let half_h = d * (fov.to_radians() * 0.5).tan();
        let half_w = half_h * aspect;
        let off = off_axis_with(rest, screen, rest, half_w, half_h);
        let sym = tracked_view_proj(Vec3::from(rest), Vec3::from(screen), fov, aspect, [0.0; 3]);
        for (a, b) in off.iter().flatten().zip(sym.iter().flatten()) {
            assert!((a - b).abs() < 1e-5, "off-axis at rest must match the symmetric frustum: {a} vs {b}");
        }
    }

    #[test]
    fn a_moved_eye_keeps_the_screen_corners_at_the_frame_edges() {
        // The whole point of the off-axis projection: the screen plane is
        // fixed in the room, so its four corners stay at the frame's four
        // corners (NDC ±1) whatever the eye does — the picture gains parallax
        // instead of sliding.
        let rest = [0.0, 0.0, 2.0];
        let screen = [0.0, 0.0, 0.0];
        let corners = [
            Vec3::new(1.0, 1.0, 0.0),
            Vec3::new(-1.0, 1.0, 0.0),
            Vec3::new(1.0, -1.0, 0.0),
            Vec3::new(-1.0, -1.0, 0.0),
        ];
        for eye in [[0.5, 0.0, 2.0], [0.0, -0.4, 1.6], [0.3, 0.2, 2.4]] {
            let m = off_axis_with(eye, screen, rest, 1.0, 1.0);
            let ndc: Vec<Vec4> = corners.iter().map(|c| project(m, *c)).collect();
            // Every corner is on screen, and the screen still fills the frame:
            // the tight frustum reaches ±1 on both axes (at whichever corner).
            for v in &ndc {
                assert!(v.x.abs() <= 1.001 && v.y.abs() <= 1.001, "{eye:?} -> {v:?}");
            }
            let (min_x, max_x) = ndc.iter().map(|v| v.x).fold((f32::MAX, f32::MIN), |(lo, hi), x| (lo.min(x), hi.max(x)));
            let (min_y, max_y) = ndc.iter().map(|v| v.y).fold((f32::MAX, f32::MIN), |(lo, hi), y| (lo.min(y), hi.max(y)));
            assert!(min_x < -0.99 && max_x > 0.99, "{eye:?} x span {min_x}..{max_x}");
            assert!(min_y < -0.99 && max_y > 0.99, "{eye:?} y span {min_y}..{max_y}");
        }
    }

    #[test]
    fn the_plain_off_axis_is_the_unit_screen_case_and_is_deterministic() {
        let a = off_axis([0.4, 0.1, 2.0], [0.0, 0.0, 0.0]);
        let b = off_axis_with([0.4, 0.1, 2.0], [0.0, 0.0, 0.0], [0.0, 0.0, 1.0], 1.0, 1.0);
        assert_eq!(a, b);
        assert_eq!(a, off_axis([0.4, 0.1, 2.0], [0.0, 0.0, 0.0]));
    }
}
