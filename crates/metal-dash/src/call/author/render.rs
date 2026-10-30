//! The live preview — `author::render`'s nine routes, moved.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::time::{Duration, Instant};

use metal_render::headless::Renderer;
use metal_render::model::{read_post, Block, Schema, EFFECTS_WIDTH};
use metal_render::preset;
use serde_json::{json, Value};

use crate::author::render::encode_jpeg;
use crate::call::{Bytes, CallError, Params};

use super::Ctx;

/// `R22.8`: the one mask `id` needs to be evaluated — itself, the operands
/// its `compose` names (recursively) and the `parent` chain above it. A
/// composed mask's region cannot be worked out without its operands, and a
/// child's alpha only reads inside its parent's silhouette, so the closure
/// is what [`metal_perform::output::apply_masks`] must see. Unknown ids drop
/// out, so a stale `mask=` cuts nothing rather than panicking.
fn mask_closure(masks: &[metal_perform::output::Mask], id: &str) -> Vec<metal_perform::output::Mask> {
    let mut out: Vec<metal_perform::output::Mask> = Vec::new();
    let mut seen: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut stack = vec![id.to_string()];
    while let Some(next) = stack.pop() {
        if !seen.insert(next.clone()) {
            continue;
        }
        let Some(m) = masks.iter().find(|m| m.id == next) else {
            continue;
        };
        out.push(m.clone());
        if let Some(c) = &m.compose {
            stack.push(c.a.clone());
            stack.push(c.b.clone());
        }
        if let Some(p) = &m.parent {
            stack.push(p.clone());
        }
    }
    out
}

fn post_json(row: &[f32]) -> Value {
    let post = read_post(row);
    json!({ "shake": post.shake, "light_leak": post.light_leak,
            "leak_color": post.leak_color,
            "ghost": post.ghost, "kaleido": post.kaleido,
            "crt": post.crt, "glitch": post.glitch,
            "posterize": post.posterize, "zoom": post.zoom,
            "fire": post.fire, "lorenz": post.lorenz,
            "vignette": post.vignette, "blur": post.blur,
            "sharpen": post.sharpen, "grain": post.grain })
}

pub fn state(ctx: &mut Ctx, _params: &Params) -> Result<Value, CallError> {
    let catalog = &ctx.state.catalog;
    let preview = &ctx.state.preview;
    let names = catalog.names();
    // Names, serialized once for the shell: the panel does not re-derive
    // the model list out of the rows' width.

    let presets: Value = names
        .iter()
        .map(|n| (n.to_string(), json!(preset::list(n))))
        .collect::<serde_json::Map<_, _>>()
        .into();
    Ok(json!({
        "rows": preview.rows,
        "post": post_json(&preview.rows[preview.selected]),
        "paused": preview.paused_at.is_some(),
        "names": names,
        "selected": preview.selected,
        "live_seq": preview.live_seq,
        "presets": presets,
        "status": preview.status,
    }))
}

/// How many float sets a gallery card cycles through (`variant=0..3`).
pub const VARIANTS: usize = 4;
/// Where each variant sits in every float's own range, as a fraction of
/// `min..max`: four distinct points, none on an edge (an edge is often the
/// degenerate end of a model, so it would show the same thing twice).
const VARIANT_T: [f32; VARIANTS] = [0.15, 0.4, 0.65, 0.9];

/// The `variant` a request asks for, wrapped into `0..VARIANTS`; `None` when
/// the request names none (the canvas's own tiles render the model's rows).
pub fn variant_of(params: &Params) -> Option<usize> {
    let k: usize = params.param("variant")?.parse().ok()?;
    Some(k % VARIANTS)
}

/// `base` with every one of the model's own floats moved to variant `k`'s
/// point in its range. Colour slots, the camera and the trailing post block
/// keep their base values: a variant varies what the model is, not its
/// palette or where the camera stands.
pub fn variant_row(schema: &Schema, base: &[f32], variant: usize) -> Vec<f32> {
    let t = VARIANT_T[variant % VARIANTS];
    let mut out = base.to_vec();
    for (i, block) in schema.blocks.iter().enumerate() {
        if let Block::Floats(floats) = block {
            let at = schema.offset(i);
            for (j, f) in floats.iter().enumerate() {
                if let Some(v) = out.get_mut(at + j) {
                    *v = f.min + (f.max - f.min) * t;
                }
            }
        }
    }
    out
}

/// The rows the grid draws: the preview's own, or with `variant=k` every
/// model's rows moved to variant `k` (see [`variant_row`]).
fn grid_rows(state: &crate::author::State, params: &Params) -> Vec<Vec<f32>> {
    let rows = &state.preview.rows;
    match variant_of(params) {
        None => rows.clone(),
        Some(k) => {
            let schemas = state.catalog.schemas();
            rows.iter()
                .enumerate()
                .map(|(i, r)| schemas.get(i).map_or_else(|| r.clone(), |s| variant_row(s, r, k)))
                .collect()
        }
    }
}

pub fn grid_jpg(ctx: &mut Ctx, params: &Params) -> Result<Bytes, CallError> {
    let tw = params.num("w", 320.0).clamp(64.0, 960.0) as u32;
    let th = params.num("h", 180.0).clamp(36.0, 540.0) as u32;
    let cols = params.num("cols", 4.0).clamp(1.0, 8.0) as u32;
    let cells = params.num("cells", 0.0).clamp(0.0, 64.0) as u32;
    let skip = params.num("skip", 0.0).clamp(0.0, 8.0) as u32;
    let time = ctx.state.preview.time();
    let rows = grid_rows(ctx.state, params);
    let (pixels, w, h) = ctx
        .state
        .renderer
        .lock()
        .expect("renderer lock")
        .grid(&rows, time, tw, th, cols, cells, skip);
    Ok(Bytes {
        kind: "image/jpeg",
        body: encode_jpeg(&pixels, w, h),
    })
}

/// Which preview row a request means: `model=<id>` resolved against the
/// state's copy of the renderer's model list, else the raw renderer index `m`.
/// The id is what the sequence editor sends (2026-10-06): its model list is
/// the manifest's (`GET /author/models/state`), which skips the registry's
/// retired tombstones, so a manifest position used as `m` drew the wrong model
/// for every model after the first tombstone (rings drew Puzzle) and had its
/// floats refused for the wrong width.
fn preview_index(state: &crate::author::State, params: &Params) -> Result<usize, CallError> {
    match params.param("model") {
        Some(id) => state
            .catalog
            .ids()
            .iter()
            .position(|m| *m == id)
            .ok_or_else(|| CallError::Bad(format!("no model {id:?}"))),
        None => Ok(params.num("m", 0.0) as usize),
    }
}

pub fn solo_jpg(ctx: &mut Ctx, params: &Params) -> Result<Bytes, CallError> {
    let index = preview_index(ctx.state, params)?;
    let w = params.num("w", 960.0).clamp(64.0, 1920.0) as u32;
    let h = params.num("h", 540.0).clamp(36.0, 1080.0) as u32;
    let (pixels, w, h) = solo_at(ctx, index, params.param("device"), params.param("mask"), row_override(params), w, h)?;
    Ok(Bytes {
        kind: "image/jpeg",
        body: encode_jpeg(&pixels, w, h),
    })
}

/// What a solo render needs from the state, copied out so the state lock is
/// released before any GPU work (see [`Author::solo_texture`]).
///
/// [`Author::solo_texture`]: crate::app::Author::solo_texture
pub struct SoloPlan {
    index: usize,
    row: Vec<f32>,
    time: f32,
    w: u32,
    h: u32,
    device: Option<String>,
    mask: Option<String>,
}

/// The node's own row a request carries (`row=<f32,f32,…>`), in place of the
/// studio's row for the model. A row of the wrong width, or with a value that
/// is not a number, is ignored: the tile then shows the studio's row, never a
/// misaligned one.
pub fn row_override(params: &Params) -> Option<Vec<f32>> {
    let row: Vec<f32> = params
        .param("row")?
        .split(',')
        .map(|v| v.trim().parse::<f32>().ok().filter(|v| v.is_finite()))
        .collect::<Option<_>>()?;
    Some(row)
}

impl SoloPlan {
    /// The plan drawn from `row` when it is as wide as the model's own row.
    pub fn with_row(mut self, row: Option<Vec<f32>>) -> SoloPlan {
        if let Some(row) = row.filter(|r| r.len() == self.row.len()) {
            self.row = row;
        }
        self
    }

    /// The clock the solo is drawn at (see [`PreviewMemo`]).
    pub fn time(&self) -> f32 {
        self.time
    }

    /// Everything the solo texture draws from except the clock: the model's
    /// row, the size, and the device's masks as they stand on disk, since the
    /// mask cut reads them (see [`solo_cut`]).
    pub fn key(&self) -> u64 {
        let mut h = DefaultHasher::new();
        (self.index, self.w, self.h, &self.device, &self.mask).hash(&mut h);
        hash_floats(&mut h, &self.row);
        if let (Some(device), Some(_)) = (self.device.as_deref(), self.mask.as_deref()) {
            format!("{:?}", crate::Outputs::load().find(device).map(|o| &o.masks)).hash(&mut h);
        }
        h.finish()
    }
}

/// The copy [`solo_pixels_render`] and the cut below work from. `NotFound` as
/// the locked path answers it when the row is missing.
pub fn pixels_plan(
    state: &crate::author::State,
    model: &str,
    device: Option<&str>,
    mask: Option<&str>,
    w: u32,
    h: u32,
) -> Result<SoloPlan, CallError> {
    let index = state
        .catalog
        .ids()
        .iter()
        .position(|m| *m == model)
        .ok_or_else(|| CallError::Bad(format!("no model {model:?}")))?;
    solo_plan_at(state, index, device, mask, w, h).ok_or(CallError::NotFound)
}

fn solo_plan_at(
    state: &crate::author::State,
    index: usize,
    device: Option<&str>,
    mask: Option<&str>,
    w: u32,
    h: u32,
) -> Option<SoloPlan> {
    if index >= state.preview.rows.len() {
        return None;
    }
    Some(SoloPlan {
        index,
        row: state.preview.rows[index].clone(),
        time: state.preview.time(),
        w,
        h,
        device: device.map(str::to_string),
        mask: mask.map(str::to_string),
    })
}

/// The CPU render of a planned solo, under the renderer's lock only.
pub fn solo_pixels_render(renderer: &mut Renderer, plan: &SoloPlan) -> (Vec<u8>, u32, u32) {
    renderer.solo(plan.index, &plan.row, plan.time, plan.w, plan.h, false)
}

/// `R22.8` point 2: `device`+`mask` are optional; with them the solo is cut
/// by that mask (and its operands/parents, `mask_closure`) exactly like the
/// device's own frame would be. No device, or an unknown mask, leaves the
/// frame whole. The devices come off disk (`Outputs::load`), the same source
/// `blackout`/`ai` read the room from.
pub fn solo_pixels_cut(mut pixels: Vec<u8>, w: u32, h: u32, plan: &SoloPlan) -> (Vec<u8>, u32, u32) {
    if let (Some(device), Some(mask)) = (plan.device.as_deref(), plan.mask.as_deref()) {
        if let Some(out) = crate::Outputs::load().find(device) {
            let closure = mask_closure(&out.masks, mask);
            if !closure.is_empty() {
                crate::output::apply_masks(&mut pixels, w, h, &closure);
            }
        }
    }
    (pixels, w, h)
}

fn solo_at(
    ctx: &mut Ctx,
    index: usize,
    device: Option<&str>,
    mask: Option<&str>,
    row: Option<Vec<f32>>,
    w: u32,
    h: u32,
) -> Result<(Vec<u8>, u32, u32), CallError> {
    let plan = solo_plan_at(ctx.state, index, device, mask, w, h).ok_or(CallError::NotFound)?.with_row(row);
    let mut renderer = ctx.state.renderer.lock().expect("renderer lock");
    let (pixels, w, h) = solo_pixels_render(&mut renderer, &plan);
    drop(renderer);
    Ok(solo_pixels_cut(pixels, w, h, &plan))
}

/// What a grid texture render needs from the state (see [`Author::grid_texture`]).
///
/// [`Author::grid_texture`]: crate::app::Author::grid_texture
pub struct GridPlan {
    rows: Vec<Vec<f32>>,
    time: f32,
    tw: u32,
    th: u32,
    cols: u32,
    cells: u32,
    skip: u32,
}

/// The clock the grid is drawn at. While the transport runs, or the preview is
/// frozen, it is the preview's own clock. Stopped and unfrozen, the clock is
/// the lanes playhead, which does not move, so a stopped grid keeps one clock
/// and [`PreviewMemo`] serves it instead of redrawing it on every request.
pub fn grid_clock(preview_time: f32, frozen: bool, running: bool, playhead_s: f32) -> f32 {
    if running || frozen {
        preview_time
    } else {
        playhead_s
    }
}

/// G5: the copy [`grid_texture`] draws. `None` when the renderers are not on
/// the painter's device, which is the same refusal [`grid_texture`] makes.
pub fn grid_plan(state: &crate::author::State, params: &Params) -> Option<GridPlan> {
    if !state.on_painter {
        return None;
    }
    let (playhead_s, running) = crate::playhead::published(metal_schema::TickRate::CANONICAL.hz() as f32);
    Some(GridPlan {
        rows: grid_rows(state, params),
        time: grid_clock(state.preview.time(), state.preview.paused_at.is_some(), running, playhead_s as f32),
        tw: params.num("w", 320.0).clamp(64.0, 960.0) as u32,
        th: params.num("h", 180.0).clamp(36.0, 540.0) as u32,
        cols: params.num("cols", 4.0).clamp(1.0, 8.0) as u32,
        cells: params.num("cells", 0.0).clamp(0.0, 64.0) as u32,
        skip: params.num("skip", 0.0).clamp(0.0, 8.0) as u32,
    })
}

impl GridPlan {
    /// The clock the grid is drawn at (see [`PreviewMemo`]).
    pub fn time(&self) -> f32 {
        self.time
    }

    /// Everything the grid draws from except the clock: the size and layout
    /// params and every planned row (variant already applied).
    pub fn key(&self) -> u64 {
        let mut h = DefaultHasher::new();
        (self.tw, self.th, self.cols, self.cells, self.skip).hash(&mut h);
        self.rows.iter().for_each(|row| hash_floats(&mut h, row));
        h.finish()
    }
}

/// How often a texture preview whose picture moves with the clock is drawn
/// again. A paused preview is memoised on its inputs alone, so it never moves;
/// a running one serves its last picture for at most this long.
pub const GRID_REFRESH: Duration = Duration::from_millis(250);
/// The solo follows the global frame cap: a clock-driven model is redrawn
/// every frame it is asked for, not at 10 Hz.
pub fn solo_refresh() -> Duration {
    crate::frame_cap::gap()
}

/// The last picture a texture preview drew, and what it drew from (see
/// [`Author::grid_texture`]). `key` hashes every input but the clock; `time`
/// is the clock it was drawn at.
///
/// [`Author::grid_texture`]: crate::app::Author::grid_texture
pub struct PreviewMemo<T> {
    key: u64,
    time: f32,
    drawn: Instant,
    frame: T,
}

impl<T: Clone> PreviewMemo<T> {
    pub fn new(key: u64, time: f32, drawn: Instant, frame: T) -> Self {
        PreviewMemo { key, time, drawn, frame }
    }

    /// The stored picture when it still answers a request for `key` at `time`
    /// and `now`: the inputs match, and either the clock matches (a paused
    /// preview) or the draw is younger than `every` (a running one).
    pub fn hit(&self, key: u64, time: f32, now: Instant, every: Duration) -> Option<T> {
        let fresh = self.time == time || now.saturating_duration_since(self.drawn) < every;
        (self.key == key && fresh).then(|| self.frame.clone())
    }
}

/// Hashes the `f32`s of a row by their bits, since `f32` is not `Hash`.
fn hash_floats(h: &mut DefaultHasher, floats: &[f32]) {
    floats.len().hash(h);
    floats.iter().for_each(|f| f.to_bits().hash(h));
}

/// G5: [`grid_jpg`]'s picture as a texture on the painter's device: the
/// renderer draws the tiles into its canvas, and the canvas is copied into a
/// frame of the painter's pool (no readback). Runs under the renderer's lock
/// only; the planned rows are the caller's copy.
pub fn grid_texture(
    renderer: &mut Renderer,
    cx: &metal_perform::gpu_compose::GpuCx,
    plan: &GridPlan,
) -> metal_perform::gpu_frame::GpuFrame {
    let (canvas, cw, ch) = renderer.grid_canvas(
        &plan.rows,
        plan.time,
        plan.tw,
        plan.th,
        plan.cols,
        plan.cells,
        plan.skip,
    );
    cx.copy_texture(&canvas, cw, ch)
}

/// G5: the solo plan for a texture, or `None` as [`solo_texture`] refuses:
/// the renderers are not on the painter's device, or the row is missing.
pub fn solo_plan(state: &crate::author::State, params: &Params) -> Option<SoloPlan> {
    if !state.on_painter {
        return None;
    }
    let index = preview_index(state, params).ok()?;
    let w = params.num("w", 960.0).clamp(64.0, 1920.0) as u32;
    let h = params.num("h", 540.0).clamp(36.0, 1080.0) as u32;
    solo_plan_at(state, index, params.param("device"), params.param("mask"), w, h)
        .map(|plan| plan.with_row(row_override(params)))
}

/// G5: the first half of [`solo_texture`]: the model draws into a new frame of
/// the painter's pool. Under the renderer's lock only.
pub fn solo_target(
    renderer: &mut Renderer,
    cx: &metal_perform::gpu_compose::GpuCx,
    plan: &SoloPlan,
) -> metal_perform::gpu_frame::GpuFrame {
    let target = cx.alloc(plan.w, plan.h);
    renderer.solo_chain_into(
        plan.index,
        &plan.row,
        plan.time,
        false,
        &[],
        &metal_render::shared::post::EstelaParams::default(),
        target.texture(),
    );
    target
}

/// G5: the second half of [`solo_texture`]: the device's mask cut, run on the
/// painter. Reads no state and holds no renderer. `target` comes back as is
/// when there is no cut to make.
pub fn solo_cut(
    paint: &mut dyn metal_perform::paint::Paint,
    target: metal_perform::gpu_frame::GpuFrame,
    plan: &SoloPlan,
) -> Option<metal_perform::gpu_frame::GpuFrame> {
    let cut = match (plan.device.as_deref(), plan.mask.as_deref()) {
        (Some(device), Some(mask)) => match crate::Outputs::load().find(device) {
            Some(out) => {
                let closure = mask_closure(&out.masks, mask);
                if closure.is_empty() {
                    None
                } else {
                    Some(metal_perform::output::apply_masks_gpu(
                        paint,
                        &target,
                        &closure,
                        &[],
                        metal_perform::layers::Composition::Overlap,
                        &Default::default(),
                        true,
                    )?)
                }
            }
            None => None,
        },
        _ => None,
    };
    Some(cut.unwrap_or(target))
}

pub fn floats(ctx: &mut Ctx, params: &Params) -> Result<Value, CallError> {
    let _device = params.param("device");
    let _layer = params.param("layer");
    let index = preview_index(&*ctx.state, params)?;
    let incoming: Vec<f32> =
        serde_json::from_slice(&params.body).map_err(|e| CallError::Bad(format!("floats: {e}")))?;
    let preview = &mut ctx.state.preview;
    if index >= preview.rows.len() {
        return Err(CallError::Bad(format!("floats: no model at index {index}")));
    }
    if incoming.len() != preview.rows[index].len() {
        return Err(CallError::Bad(format!(
            "floats: wrong width for {:?}: got {}, the schema wants {}",
            params.param("model").unwrap_or("?"),
            incoming.len(),
            preview.rows[index].len()
        )));
    }
    preview.rows[index] = incoming;
    Ok(json!({ "ok": true }))
}

pub fn effects_retired(_ctx: &mut Ctx, _params: &Params) -> Result<Value, CallError> {
    Err(CallError::Bad(
        "retired: effects are part of /floats now".into(),
    ))
}

pub fn select(ctx: &mut Ctx, params: &Params) -> Result<Value, CallError> {
    let index = params.num("m", 0.0) as usize;
    let preview = &mut ctx.state.preview;
    if index >= preview.rows.len() {
        return Err(CallError::Bad("select: no such model".into()));
    }
    preview.selected = index;
    Ok(json!({ "selected": index }))
}

pub fn reset(ctx: &mut Ctx, params: &Params) -> Result<Value, CallError> {
    let index = params.num("m", 0.0) as usize;
    if index >= ctx.state.preview.rows.len() {
        return Err(CallError::Bad("reset: no such model".into()));
    }
    ctx.state.preview.rows[index] = ctx.state.catalog.schemas()[index].defaults();
    Ok(json!({ "floats": ctx.state.preview.rows[index] }))
}

pub fn pause(ctx: &mut Ctx, _params: &Params) -> Result<Value, CallError> {
    let preview = &mut ctx.state.preview;
    preview.paused_at = match preview.paused_at {
        Some(frozen) => {
            preview.started =
                web_time::Instant::now() - std::time::Duration::from_secs_f32(frozen);
            None
        }
        None => Some(preview.time()),
    };
    Ok(json!({ "paused": preview.paused_at.is_some() }))
}

pub fn preset(ctx: &mut Ctx, params: &Params) -> Result<Value, CallError> {
    let _device = params.param("device");
    let _layer = params.param("layer");
    let index = params.num("m", 0.0) as usize;
    if index >= ctx.state.preview.rows.len() {
        return Err(CallError::Bad("preset: no such model".into()));
    }
    let name = params.param("name").unwrap_or_default().to_string();
    let model = ctx.state.catalog.names()[index];
    match params.param("op") {
        Some("save") => {
            let post = Default::default();
            let row = ctx.state.preview.rows[index].clone();
            ctx.state.preview.status = match preset::save(model, &name, &row, &post) {
                Ok(()) => format!("guardado: {model}/{name}"),
                Err(e) => format!("error al guardar: {e}"),
            };
        }
        Some("load") => match preset::load(model, &name) {
            Ok(p) if p.floats.len() == ctx.state.preview.rows[index].len() => {
                ctx.state.preview.rows[index] = p.floats;
                ctx.state.preview.status = format!("cargado: {model}/{name}");
            }
            Ok(p) if p.floats.len() + EFFECTS_WIDTH == ctx.state.preview.rows[index].len() => {
                let mut floats = p.floats;
                floats.push(p.post.shake);
                floats.push(p.post.light_leak);
                floats.push(p.post.ghost);
                floats.push(p.post.kaleido);
                floats.push(p.post.crt);
                floats.push(p.post.glitch);
                floats.push(p.post.posterize);
                floats.push(p.post.zoom);
                floats.push(p.post.fire);
                floats.push(p.post.lorenz);
                floats.push(p.post.vignette);
                floats.push(p.post.blur);
                floats.push(p.post.sharpen);
                floats.push(p.post.grain);
                floats.extend(p.post.leak_color.iter().copied());
                ctx.state.preview.rows[index] = floats;
                ctx.state.preview.status = format!("cargado (formato anterior): {model}/{name}");
            }
            Ok(_) => ctx.state.preview.status = "el preset no tiene el ancho de este modelo".into(),
            Err(e) => ctx.state.preview.status = format!("error al cargar: {e}"),
        },
        _ => return Err(CallError::Bad("preset: op must be save or load".into())),
    }
    let row = ctx.state.preview.rows[index].clone();
    Ok(json!({
        "status": ctx.state.preview.status,
        "floats": row,
        "post": post_json(&row),
        "presets": preset::list(model),
    }))
}

#[cfg(test)]
mod tests {
    static VARIANT_FLOATS: [metal_render::model::FloatSpec; 2] = [
        metal_render::model::spec("a", 0.0, 1.0, 0.5),
        metal_render::model::spec("b", 10.0, 30.0, 20.0),
    ];
    static VARIANT_SCHEMA: Schema = Schema { blocks: &[Block::Floats(&VARIANT_FLOATS), Block::ColorSlot("c", &metal_render::model::WHITE4)] };

    fn with_query(k: &str, v: &str) -> Params {
        let mut p = Params::default();
        p.query.insert(k.into(), v.into());
        p
    }

    fn plan(row: Vec<f32>) -> SoloPlan {
        SoloPlan { index: 0, row, time: 0.0, w: 8, h: 8, device: None, mask: None }
    }

    /// A request's `row` replaces the studio's row for that tile, but only
    /// when it is as wide as the model's own row and every value is a number.
    #[test]
    fn a_requests_row_replaces_the_studio_row_only_when_it_fits() {
        assert_eq!(row_override(&Params::default()), None);
        assert_eq!(row_override(&with_query("row", "0.5, 2,3")), Some(vec![0.5, 2.0, 3.0]));
        assert_eq!(row_override(&with_query("row", "0.5,x")), None);
        assert_eq!(row_override(&with_query("row", "0.5,NaN")), None);
        let base = vec![1.0, 1.0, 1.0];
        assert_eq!(plan(base.clone()).with_row(Some(vec![0.5, 2.0, 3.0])).row, vec![0.5, 2.0, 3.0]);
        assert_eq!(plan(base.clone()).with_row(Some(vec![0.5])).row, base, "a row of the wrong width is ignored");
        assert_eq!(plan(base.clone()).with_row(None).row, base);
        let (a, b) = (plan(base.clone()), plan(base).with_row(Some(vec![0.5, 2.0, 3.0])));
        assert_ne!(a.key(), b.key(), "the memo tells the two rows apart");
    }

    #[test]
    fn variant_of_wraps_and_is_absent_without_the_param() {
        assert_eq!(variant_of(&Params::default()), None);
        assert_eq!(variant_of(&with_query("variant", "2")), Some(2));
        assert_eq!(variant_of(&with_query("variant", "6")), Some(2), "wraps into 0..4");
        assert_eq!(variant_of(&with_query("variant", "x")), None);
    }

    #[test]
    fn four_variants_are_four_distinct_float_sets_inside_the_range() {
        let base = VARIANT_SCHEMA.defaults();
        let sets: Vec<Vec<f32>> = (0..VARIANTS).map(|k| variant_row(&VARIANT_SCHEMA, &base, k)).collect();
        for (k, row) in sets.iter().enumerate() {
            assert_eq!(row, &variant_row(&VARIANT_SCHEMA, &base, k), "deterministic");
            assert!((0.0..=1.0).contains(&row[0]) && (10.0..=30.0).contains(&row[1]), "in range: {row:?}");
            assert_eq!(&row[2..], &base[2..], "colour slots keep their base values");
        }
        for a in 0..VARIANTS {
            for b in a + 1..VARIANTS {
                assert_ne!(sets[a][..2], sets[b][..2], "variants {a} and {b} differ");
            }
        }
    }

    use super::*;
    use metal_perform::output::Mask;

    fn square(id: &str) -> Mask {
        Mask {
            id: id.into(),
            name: id.into(),
            points: vec![[0.25, 0.25], [0.75, 0.25], [0.75, 0.75], [0.25, 0.75]],
            ..Default::default()
        }
    }

    fn lit(pixels: &[u8], w: u32, x: u32, y: u32) -> bool {
        pixels[((y * w + x) * 4) as usize] > 0
    }

    /// `R22.8` punto 2: the solo cut by a mask keeps what is inside it and
    /// blacks what is not — the same `apply_masks` the device frame gets,
    /// fed the chosen mask's own closure.
    #[test]
    fn solo_with_a_mask_blacks_out_outside_it() {
        let masks = [square("mask-1")];
        let closure = mask_closure(&masks, "mask-1");
        assert_eq!(closure.len(), 1);
        let (w, h) = (100u32, 100u32);
        let mut px = vec![255u8; (w * h * 4) as usize];
        crate::output::apply_masks(&mut px, w, h, &closure);
        assert!(lit(&px, w, 50, 50), "inside the mask went dark");
        assert!(!lit(&px, w, 5, 5), "outside the mask stayed lit");
    }

    /// A composed mask pulls in its operands, and a child pulls in its
    /// parent: the closure is exactly what `apply_masks` needs to evaluate
    /// the chosen mask.
    #[test]
    fn the_closure_carries_operands_and_parents() {
        let mut operand = square("a");
        let mut other = square("b");
        let parent = square("p");
        let mut child = square("c");
        child.parent = Some("p".to_string());
        operand.compose = None;
        other.compose = None;
        let mut composed = square("mix");
        composed.compose = Some(metal_perform::output::Compose {
            op: metal_perform::output::BoolOp::Intersect,
            a: "a".into(),
            b: "b".into(),
        });
        let masks = vec![operand, other, parent, child, composed];
        let mix = mask_closure(&masks, "mix");
        let mut ids: Vec<&str> = mix.iter().map(|m| m.id.as_str()).collect();
        ids.sort();
        assert_eq!(ids, vec!["a", "b", "mix"]);
        let kid = mask_closure(&masks, "c");
        let mut child_ids: Vec<&str> = kid.iter().map(|m| m.id.as_str()).collect();
        child_ids.sort();
        assert_eq!(child_ids, vec!["c", "p"]);
        assert!(mask_closure(&masks, "ghost").is_empty());
    }

    #[test]
    fn a_stopped_grid_keeps_one_clock_so_the_memo_serves_it() {
        // Stopped and not frozen: the clock is the playhead, whatever the
        // preview's elapsed time says, so two requests share one clock.
        let first = grid_clock(12.5, false, false, 3.0);
        let later = grid_clock(47.0, false, false, 3.0);
        assert_eq!(first, later);
        let t0 = Instant::now();
        let memo = PreviewMemo::new(7, first, t0, "frame");
        assert_eq!(memo.hit(7, later, t0 + Duration::from_secs(60), GRID_REFRESH), Some("frame"));
        // Running, or frozen, the preview's own clock moves the picture.
        assert_eq!(grid_clock(12.5, false, true, 3.0), 12.5);
        assert_eq!(grid_clock(12.5, true, false, 3.0), 12.5);
    }

    #[test]
    fn a_preview_memo_hits_on_the_same_inputs_and_clock_or_a_recent_draw() {
        let t0 = Instant::now();
        let every = Duration::from_millis(100);
        let memo = PreviewMemo::new(7, 1.0, t0, "frame");
        // A paused preview: the same clock, whatever the age of the draw.
        assert_eq!(memo.hit(7, 1.0, t0 + Duration::from_secs(60), every), Some("frame"));
        // A running one: a draw younger than the refresh interval is served...
        assert_eq!(memo.hit(7, 2.0, t0 + Duration::from_millis(50), every), Some("frame"));
        // ...and an older one is not.
        assert_eq!(memo.hit(7, 2.0, t0 + Duration::from_millis(150), every), None);
        // Any change to the inputs is a miss, even on the same clock.
        assert_eq!(memo.hit(8, 1.0, t0, every), None);
    }
}
