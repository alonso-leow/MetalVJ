//! The three systems every model shares, implemented once: the 4-colour
//! gradient slot, the camera block, and the post chain (light leaks, then
//! shake) that runs after whichever model just drew. `PRELUDE` is prepended
//! to every model's own WGSL so `grad4`/`hash11`/`ColorSlot` are always in
//! scope without a module system WGSL does not have.

pub mod backdrop;
pub mod brush;
pub mod footage;
pub mod fullscreen;
pub mod kit;
pub mod line2d;
pub mod mesh;
pub mod scaffold;
pub mod parallax;
pub mod post;
pub mod sim;
pub mod stutter;
pub mod surface;
pub mod trail;
pub mod warp;
pub mod weave;

pub const PRELUDE: &str = include_str!("../../shaders/shared.wgsl");

/// Matches `ColorSlot` in shared.wgsl: 4 colours, RGB each, padded to vec4
/// because a uniform's array stride is 16 bytes regardless.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ColorSlotGpu {
    pub c: [[f32; 4]; 4],
}

impl ColorSlotGpu {
    /// `f` is 12 floats: 4 colours x (r, g, b), each 0..1.
    pub fn from_floats(f: &[f32]) -> Self {
        debug_assert_eq!(f.len(), 12, "a colour slot is 4 colours x rgb");
        let mut c = [[0.0f32; 4]; 4];
        for i in 0..4 {
            c[i] = [f[i * 3], f[i * 3 + 1], f[i * 3 + 2], 1.0];
        }
        ColorSlotGpu { c }
    }

    pub fn solid(r: f32, g: f32, b: f32) -> Self {
        ColorSlotGpu { c: [[r, g, b, 1.0]; 4] }
    }

    /// `grad4` on the CPU — the same piecewise walk `shared.wgsl` does.
    ///
    /// Only one thing needs this, and it needs it to be *exactly* the same
    /// arithmetic: a model whose clear colour has to match a colour its own
    /// fragment shader produces. Oracle is that model — its lid is the same
    /// skin the frame is cleared to, so a closed eye disappears into the
    /// background rather than into a black rectangle. An approximation here
    /// would show up as a seam around the eyelid at every blink.
    pub fn grad4(&self, t: f32) -> [f32; 3] {
        let tt = t.clamp(0.0, 1.0) * 3.0;
        let seg = tt.floor().clamp(0.0, 2.0);
        let f = tt - seg;
        let i = seg as usize;
        let (a, b) = (self.c[i], self.c[i + 1]);
        [
            a[0] + (b[0] - a[0]) * f,
            a[1] + (b[1] - a[1]) * f,
            a[2] + (b[2] - a[2]) * f,
        ]
    }

    /// The clear colour for a pass whose background is one of its own colour
    /// slots. `Rgba8UnormSrgb` targets encode on write, so the linear value
    /// handed to `LoadOp::Clear` is the same number the shader writes — no
    /// gamma conversion here, deliberately.
    pub fn clear_at(&self, t: f32) -> wgpu::Color {
        let c = self.grad4(t);
        wgpu::Color { r: c[0] as f64, g: c[1] as f64, b: c[2] as f64, a: 1.0 }
    }
}

/// Matches `CameraBlock` in shared.wgsl: pos.xyz + target.xyz + fov degrees,
/// the 7-float shared camera block every 3D model can opt into.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct CameraGpu {
    pub pos_pad: [f32; 4],
    pub target_fov: [f32; 4],
}

impl CameraGpu {
    /// `f` is 7 floats: pos.xyz, target.xyz, fov_degrees.
    pub fn from_floats(f: &[f32]) -> Self {
        debug_assert_eq!(f.len(), 7, "a camera block is pos3 + target3 + fov1");
        CameraGpu {
            pos_pad: [f[0], f[1], f[2], 0.0],
            target_fov: [f[3], f[4], f[5], f[6]],
        }
    }

    pub fn pos(&self) -> glam::Vec3 {
        glam::Vec3::from_slice(&self.pos_pad[..3])
    }

    pub fn target(&self) -> glam::Vec3 {
        glam::Vec3::from_slice(&self.target_fov[..3])
    }

    pub fn fov_radians(&self) -> f32 {
        self.target_fov[3].to_radians()
    }

    /// The view-projection matrix a model's vertex shader needs. `up` is
    /// always +Y; none of the 10 models bank the camera.
    pub fn view_proj(&self, aspect: f32) -> glam::Mat4 {
        let view = glam::Mat4::look_at_rh(self.pos(), self.target(), glam::Vec3::Y);
        let proj = glam::Mat4::perspective_rh(
            self.fov_radians().clamp(0.05, std::f32::consts::PI - 0.05),
            aspect,
            0.05,
            100.0,
        );
        proj * view
    }
}

/// A vertex/fragment pair whose source is `shared::PRELUDE` followed by a
/// model's own WGSL — the string-concatenation stand-in for a module system.
pub fn build_shader(device: &wgpu::Device, label: &str, model_wgsl: &str) -> wgpu::ShaderModule {
    let source = format!("{PRELUDE}\n{model_wgsl}");
    device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(label),
        source: wgpu::ShaderSource::Wgsl(source.into()),
    })
}
