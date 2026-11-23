# Portal transition + standard float names

Branch `feat/portal`, uncommitted on `3b1fd75` (origin/master, 2026-11-17). Worktree `~/Projects/mvj-portal`, target
`~/.cache/mvj-targets/splat`.

## 1. What the float names look like today (research)

104 models in `data/schema.json`. Shared tails (`sig_*`, `col_*`, fx, `grade_*`,
`mat_*`) are the same on every model and are not counted below.

**The reference names are the kits** (`metal-render/src/shared/kit.rs`):

| kit | floats |
|---|---|
| `CAM2D` (≈60 2D models) | `zoom pan_x pan_y turn drift` |
| `CAM3D` (object, catenoid, particles, fineline, bulb) | `cam_yaw cam_pitch cam_roll cam_distance cam_fov cam_orbit cam_height cam_dolly cam_shake` |
| `LOOK` | `shine contrast opacity background_level tone saturation` |
| `TWIST` | `warp_amount warp_freq mirror pulse` |
| H4 head track (12 models) | `cam_x cam_y cam_z` |

Rule from the kits: lowercase snake_case, **English order** (modifier first:
`scan_speed`, not `speed_scan`), a family prefix for grouped floats (`cam_`,
`grade_`, `mat_`, `glow_`, `bg_`), `_x/_y/_z` and `_1…_n` suffixes for
components.

**3D viewpoint: three incompatible schemes.**

- 12 models (chamber, membrane, oracle, turing, vaporgrid, river, room,
  parallax-cave/sky/fields/city, scene3d) carry an *unnamed* 7-float
  `Block::Camera` (one `camera` param, not controllable float by float) plus
  `cam_x/y/z`. No named yaw/pitch/dolly.
- 5 kit models have `cam_yaw…cam_shake` but no `cam_x/y/z`.
- Strays: magnet-spikes `cam_dist`; scene3d `rot_x/orbit/distance/fov`;
  effigies `pitch/yaw/roll`; spectral-ridges `yaw/tilt`.

**Odd names, by cause.** Almost all come from the word-by-word Spanish→English
migration (`scripts/migrate-param-names.py`), which kept Spanish word order:

1. *Spanish order* (`noun_modifier`): `speed_scan` (velocidad_escaneo),
   `width_shine`, `angle_cell`, `width_edge`, `shine_center`, `shine_edge`,
   `contrast_smoke`, `drift_color` (6 models) while metaballs has
   `color_drift`, `turn_mesh`, `mass_central`, `branch_long`, `wake_long`,
   `speed_background`, `scale_background`, `variation_palette`…
2. *Count spelled five ways*: `n_layers n_eyes n_part n_objects n_sources
   n_folds n_bands num_arcs emitters_n splat_n slit_n` vs `pole_count`/`count`.
3. *Spanish left over*: slots `raiz patron pliegue_color ojo_brillo paneles
   colores colores2 punto tintas calor bolas frente medio tono_a tono_b linea
   agua banda`; floats `graves medios agudos` (glyph-stele).
4. *Abbreviations*: `off_x/off_y` (6) vs `offset_x/offset_y` (4), `rot`,
   `rot_speed`, `kaleid`, `mix_f`, `scale_f`, `p_red`, `prob`, `fog_c`,
   `size_var`, `crack_len`, `block_w/h`, `window_w/h`, `cheb`, `sat_img`,
   `contrast_img`, `shine_img`, `osc_off`, `drift_v`, `iter_swirl`,
   `pressure_iter`, `steps_frame`.
5. *A float and a colour slot with one name* in the same model (escape,
   cyberspace `neon`, oracle/monster `iris`, rings `ball`, stained-glass
   `lead`, gemsmoke `body`/`shine`, …): two params named alike in the panel and
   for any by-name binding.
6. Known and left: own `zoom`/`grain`/`vignette`/`blur`/`glitch`… shadow the
   same-named shared fx floats in `row_layout` lookups (the kit `zoom` is the
   reference, so it stays); physics terms `g dt cfl ucr m1 n1_1` stay.

## 2. Standard (binding)

- **3D pose, on every 3D gen node**: `cam_x cam_y cam_z cam_yaw cam_pitch
  cam_dolly`. Position offset in world units; yaw/pitch in radians (positive
  yaw looks left, i.e. a right-handed turn about +Y); positive dolly pulls
  back along the view, eye and target together (the kit adds it to the orbit
  distance). Camera-block models turn at the eye; kit
  (orbit) models keep orbiting the subject. `metal_schema::POSE_NAMES` is the
  list (`pose_indices` finds them in a manifest model); a model is "3D" for the engine iff it declares all six. A test fails a
  model that has a camera but not the full set.
- Renames keep positions (rules 1–2). 137 floats and 55 colour slots were
  renamed (all of groups 1–5 above). Old names whose name no longer exists
  anywhere went into `param_aliases::PARAM_NAME_ALIASES` (the Spanish entries
  that pointed at them were retargeted, so `ancho_brillo` → `shine_width`
  directly). The 19 slots renamed off a float's name (`escape` →
  `escape_color`, …) alias only colour values (`SLOT_ALIASES`,
  `slot_alias_keys`), because the old name is still a live float. Versioned
  `data/` (loop templates, dataset specs/examples) was rewritten, and dataset
  examples of the 17 widened 3D models got the three pose defaults inserted.
  Two examples (`04-p2`, `65-p1`) were already stale against the old schema
  and were left as they were.
- Pending: `web/wasm/data/schema.json` (stale before this branch) regenerates
  with `scripts/build-wasm.sh`; Colourist weights trained on the 17 widened
  models stop fitting (`Grade::fits`) until retrained.

## 3. Portal transition

`transition::Kind::Portal`, configured like any transition
(`transition_in`/`transition_out`, presets in `data/transitions/`), plus
`angle` (90 or 180) and `via` (`auto`/`through`/`reveal`); `dir` left/right is
the side.

- **through** (origin 3D): the outgoing node is the *host*: its camera pans
  `angle` to the side (engine adds to `cam_yaw`), a portal frame in its world
  holds the incoming node's live picture, then the camera dollies
  (`cam_dolly`) through it.
- **reveal** (origin 2D → 3D): the same clip played backwards with the
  incoming node as host: the 2D picture fills the screen, zooms out into a
  framed portal still rendering, then the 3D host pans back to centre.
- **auto**: through when the outgoing node is 3D, reveal when only the
  incoming one is, through with a flat (re-projected) host for 2D→2D.
- Both halves are per-node passes over the same virtual camera: the host cuts
  a hole where the portal is (and draws its rim), the guest is drawn only
  inside it, so layer order does not matter. CPU pass first; the GPU pass
  falls back to it (`transition_texture` → `None`).

Built: `transition::{Via, PortalPart, portal_camera, apply_portal,
portal_parts}`, `engine::{portal_frame, apply_portal_pose}`, presets
`portal-izquierda`, `portal-derecha`, `portal-atras` (180°), and side/angle/host
choices on the properties transition page. Open: a GPU pass (the CPU pass reads
the tile back), and a look in the app with real models.

**Escalate if** a model's dolly units make the host visibly overshoot the
portal: a per-model dolly scale would be a new float.
