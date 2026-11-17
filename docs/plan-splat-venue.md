# Gaussian splats in the 3D venue, with live fixtures

Date: 2026-11-16. Branch `feat/splat-venue` (worktree `~/Projects/mvj-splat`).

## Scope

- **Splat files already exist.** They are made with external apps (Brush, Postshot, Polycam, Luma, KIRI, gsplat, Blender 5.3). MetalVJ only imports the file. Training is not in this plan.
- **The venue 3D view must show three things in one room:**
  1. splats;
  2. the venue's own geometry: surfaces/screens, objects, projectors with their frustums;
  3. **live fixtures**: pan, tilt, dimmer, colour, zoom and strobe from the show, through the slew model (`FixtureState::step`).

## What Blender 5.3 does (research, 2026-11)

- **Native since 5.3** (alpha/beta in Oct 2026; 5.0–5.2 needed the KIRI 3DGS add-on). It imports `.ply`, `.spz` and USD splats as a *point cloud type* with these attributes:
  - `position`
  - `scaling` (vec3)
  - `rotation` (quat)
  - `radiance` (SH: `radiance:base` + `radiance:sh`)
  - opacity
- **No export yet.**
- **EEVEE/Workbench** draw each gaussian as a camera-facing quad, back to front, with *dithered* transparency. Splats are ordinary depth-tested objects there, so meshes occlude them and they occlude meshes. The dithering is noisy and needs many samples.
- **Cycles** ray-traces splats: at the max-opacity point along the ray, opacity is used as a probability. Reflections, indirect light and shadows from meshes work.
- **Lighting:** the default material is emissive, so lights don't affect splats. Relighting uses a diffuse shader with a normal estimated from the smallest scale axis, and is early-stage.
- **No separate pass is required in Blender.** Its renderers treat splats as scene objects.

What we take from it:

1. **Separate layer.** A real-time viewport with an egui painter can't interleave splats and meshes per pixel. So splats are **their own layer**, depth-tested against a **depth-only prepass of the venue geometry** (surfaces, objects, fixture bodies). Then:
   - the egui-painted opaque faces go first;
   - the splat layer is composited on top: pixels behind a wall were discarded, so the wall shows, and splats in front of a wall cover it;
   - beams, frustums, handles and labels go last.
   
   This gives the same visible result as Blender's depth-tested EEVEE, without dithering.
2. **Lighting is baked.** Splats stay emissive. Fixture beams are an additive layer over them, not lighting on them. An optional "lit" tint (beam hit near a splat brightens it) is a later step.
3. **Import conventions:** `.ply` follows the INRIA 3DGS layout. Blender's PLY import auto-detects Z-forward/-Y-up. We store an `up` choice (`y` | `-y` | `z`) on the object and default to `-y` (COLMAP/INRIA captures). The user fixes it with the object's yaw and size like any other object.

## Data

- **`metal_perform::venue::ObjectForm::Splat`:**
  ```
  Splat { file: String, up: String /* "-y" default */, budget: u32 /* 0 = auto */, crop: Option<[[f32;3];2]> }
  ```
  - The object's `at`/`yaw`/`size` place it: `size` is the box the splat bounds are fitted into (uniform scale by the largest axis).
  - `occludes` defaults to false for splats.
  - The file path is relative to the data root: `data/splats/<name>.ply`. The import op copies the file there.
  - Ray hits (`Venue::hit`) ignore splats.
- **`metal_geom::splat`:** `Splats { pos, scale, rot, opacity, rgb }` (SoA, f32).
  - Loaders: `.ply` (binary_little_endian, INRIA fields `x y z f_dc_0..2 opacity scale_0..2 rot_0..3`, `f_rest_*` read and skipped) and `.splat` (32-byte antimatter records).
  - Activations at load: `exp(scale)`, `sigmoid(opacity)`, normalised quat, `rgb = 0.5 + 0.28209479 * f_dc` clamped.
  - Helpers: `bounds`, `decimate(budget)` (importance = opacity × volume, deterministic), `covariance3(i)`, `project(i, view, proj, viewport) -> Option<Ellipse>` (EWA 2D covariance → centre, axes, depth), and `sort_back_to_front(depths)`.
- **Live fixtures:** `metal_perform::live`, a process-wide store.
  - **Writes:** the engine records the `Signal` each fixture head was driven with this frame (`fixture id → Signal + time`).
  - **Reads:** `GET /venue/state` turns each live `Signal` into `commanded`:
    - pan/tilt in degrees through the profile's channel physical range;
    - `level` → dimmer;
    - `rgb()` → rgb;
    - `zoom` → beam degrees (profile range, else the authored beam);
    - `strobe`.
  - It then steps `actual` with `FixtureState::step(limits, dt)` and keeps the states between calls.
  - Fixtures with no live signal return the authored pose with `live: false`.
  - Reply: `{venue, fixtures:[{fixture, commanded, actual, strobe, live}]}`.

## View

- **Fixtures (CPU, every backend):** the viewport polls `GET /venue/state` at most 10×/s and draws:
  - the body: yoke + head turned by `actual.pan/tilt`;
  - the beam cone: half-angle from `actual.zoom`, colour `rgb × dimmer`, translucent fill + edge lines, length to the first hit (`Venue::hit`) or 12 m;
  - a hit spot ellipse on the surface/floor;
  - strobe flicker (the cone toggles at the strobe rate).
  
  Projectors keep their frustum. A projector's frustum is drawn to its first surface hit.
- **Splats, CPU layer (web/glow, and the fallback):**
  - project the decimated splats (budget about 60k on web, 150k desktop CPU);
  - sort back to front;
  - draw each as a textured quad (a 64×64 gaussian falloff texture registered once) in an `egui::Mesh`;
  - interleave with the painter's faces by depth: faces and splat batches go into one depth-sorted list;
  - re-sort only when the camera moved (the cached order is kept otherwise).
- **Splats, GPU layer (desktop wgpu backend):** an `egui_wgpu::CallbackTrait` paint callback.
  - `prepare` renders into an offscreen colour + depth target: a depth-only prepass of the venue triangles, then instanced splat quads with depth test and no depth write, premultiplied alpha, back-to-front order.
  - The sort is CPU, on a worker thread keyed by camera; GPU radix sort comes later.
  - `paint` blits the offscreen texture into the egui pass inside the viewport rect.
  - Used only when the window painter is wgpu (runtime flag), otherwise the CPU layer.

## Waves

1. **(parallel)**
   - **A:** `metal_geom::splat` loaders and projection.
   - **B:** `ObjectForm::Splat` and the import op.
   - **C:** the live fixture store and `/venue/state`.
   - **D:** live fixture bodies, beams and hits in the viewport.
2. **(after A+B)**
   - **E:** the CPU splat layer in the viewport.
   - **F:** the GPU splat layer on desktop.
3. **Merge, tests, a headless shot of a venue with a splat, a moving head and a screen.**

## Open

- GPU radix sort and SH degree > 0.
- `.spz`/`.sog` (compressed) import.
- Beam light tinting splats.
- Splats inside generated model views (`metal-render`), not only in the editor viewport.

## Status (2026-11-16)

All waves are landed on `feat/splat-venue`.

Tests:
- `metal-geom` splat tests: 9 pass.
- `metal-perform`: 941 pass.
- `metal-dash` viewport, splat and venue tests pass, and the WGSL validates through naga.

Headless shots:
- `shots_venue_splat` (CPU layer) and `shots_venue_splat_gpu` (wgpu paint-callback layer through kittest's wgpu renderer) render the same room: a splat sculpture on a stage, two live moving heads crossing on it, a wash, a projector frustum to the screen, and a side wall.
- Run them with:
  ```
  FW_SHOTS_DIR=… cargo test -p metal-dash --lib -F shots shots_venue_splat -- --test-threads=1
  ```

Notes:
- The room the app shows is `viewport::paint_room`, called from `venue_canvas`. That is where `/venue/state` is polled. `paint_interactive` is not called by the shell.
- Beam cones are drawn as one unfeathered `egui::Mesh`. As separate convex polygons, their needle-thin slices got anti-aliasing spikes.
- Live poses are recorded only on the `venue.fixture` node path (`wire::fixture_signal_packet`). Heads driven only through an `out.*` head list do not report yet.

Still open (beyond the list above):
- Ellipse sizes are wrong in ortho mode (the Jacobian is perspective only).
- The splat depth test uses the centre depth per splat.
- A failed load is not retried until restart.
- No `splat` entry in the agent tool schema.

## Follow-up (2026-11-17, branch `feat/splat-open`)

Closed:
- **Ortho:** `Splats::project` uses the affine Jacobian when the projection is orthographic.
- **SH degree 1–3:** `f_rest_*` are kept (`Splats::sh`, `sh_degree`). `colour_from(i, eye)` evaluates the 3DGS basis, and the CPU layer uses it. The GPU layer keeps base colour.
- **`.spz` import (v2/v3):** gzip, with field orders as in Niantic's `load-spz.cc`: "smallest three" quaternions in sign-magnitude, w..x from the low bits; channel-interleaved SH. The venue `splat` op accepts `.spz`.
- **Retry:** a failed load retries when the file's mtime changes, or every 5 s (native).
- **Browser:** splats load through `metal_vfs::read` (`Splats::from_bytes`).
- **GPU sort:**
  - The GPU layer builds world-space splat data on the CPU only when the clouds or their placement change.
  - Per frame, a compute pass projects every splat (same maths and constants as `Splats::project`, shared from `metal_geom::splat`), and a bitonic sort orders them far to near. The quads read the sorted order from storage buffers.
  - The layer turns on only when `splat_gpu::supported(device)`. With more than 8M splats it declines, and the CPU layer draws.
  - When the GPU layer draws, the CPU no longer projects.
- **Beam light on splats:** `splat::beam_light`/`lit` brighten splats inside a live beam's cone, before its first hit. The CPU layer applies it to the cached projection without re-sorting; the GPU layer applies it in the compute pass. The beams come from `viewport_fixtures::beam_lights`, the same cones `draw_beams` draws.
- **Live poses from every drive path:** group nodes, frame-sampled heads and `out.*` sACN/Art-Net head lists record into `fixture_live`.
- **Agent tool:** `import_splat(venue, file, at?, yaw?, size?, up?, budget?)` posts `op:"splat"`, and `budget` is honoured by the op.
- **es locale:** the duplicate "Palette" key is gone.

Still open:
- SH on the GPU layer.
- Splats in `metal-render` model views.
- The per-splat centre depth test.
- The browser build's pre-existing compile errors outside the splat code (`call/transmit.rs`, `call/mod.rs`, `lanes.rs`, `stage_frame.rs`).
