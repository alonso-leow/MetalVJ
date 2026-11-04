//! The shell's two motions.
//!
//! 1. A panel sliding in or out of the row (`slide`): an eased 0→1 that the
//!    region layout turns into the panel's width.
//! 2. A list refresh (`begin` / `end` / `scoped`): when what a panel shows
//!    changes key (another folder, another selection), the rows on view fade
//!    out one by one from the last to the first, the new key takes over, and
//!    its rows fade in one by one from the first to the last. Rows take their
//!    turn in paint order through [`scoped`], so any list joins by wrapping
//!    its rows.
//!
//! Tests run without motion: a key change is shown at once.

use egui::{Context, Id, Ui};

/// Seconds a panel takes to slide in or out.
const SLIDE_SECS: f32 = 0.22;
/// Seconds one row takes to fade, and the longest the whole stagger may last.
const FADE_SECS: f32 = 0.14;
const SPAN_SECS: f32 = 0.22;
/// The gap between two rows' starts, at most.
const STEP_SECS: f32 = 0.03;

#[derive(Clone, Copy, Default)]
struct Mem {
    shown: u64,
    /// 0 rest, 1 fading out, 2 fading in.
    phase: u8,
    start: f64,
    count: usize,
    init: bool,
}

#[derive(Clone, Copy)]
struct Active {
    phase: u8,
    elapsed: f32,
    delay: f32,
    count: usize,
    next: usize,
}

fn enabled() -> bool {
    !cfg!(test)
}

/// Whether a key change waits for its fade-out (never in tests).
pub fn deferring() -> bool {
    enabled()
}

/// Ease in-out, 0..1.
pub fn ease(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The eased openness of a panel: 0 hidden … 1 fully open.
pub fn slide(ctx: &Context, id: Id, open: bool) -> f32 {
    if !enabled() {
        return if open { 1.0 } else { 0.0 };
    }
    ctx.animate_bool_with_time_and_easing(id, open, SLIDE_SECS, egui::emath::easing::cubic_in_out)
}

/// Starts a list pass for `id` showing `key`. Returns the key to paint: with
/// `defer` it is the old key until the fade-out ends; without, it is `key`
/// at once and only the fade-in runs.
pub fn begin(ctx: &Context, id: Id, key: u64, defer: bool) -> u64 {
    if !enabled() {
        return key;
    }
    let now = ctx.input(|i| i.time);
    let mid = id.with("motion");
    let mut mem: Mem = ctx.data(|d| d.get_temp(mid)).unwrap_or_default();
    if !mem.init {
        mem = Mem { shown: key, init: true, ..Mem::default() };
    }
    if mem.phase == 0 && mem.shown != key {
        if defer && mem.count > 0 {
            mem.phase = 1;
        } else {
            mem.shown = key;
            mem.phase = 2;
        }
        mem.start = now;
    }
    let count = mem.count.max(1);
    let delay = STEP_SECS.min(SPAN_SECS / count as f32);
    let total = delay * count as f32 + FADE_SECS;
    let mut elapsed = (now - mem.start) as f32;
    if mem.phase == 1 && elapsed >= total {
        mem.shown = key;
        mem.phase = 2;
        mem.start = now;
        elapsed = 0.0;
    }
    if mem.phase == 2 && elapsed >= total {
        mem.phase = 0;
    }
    if mem.phase != 0 {
        ctx.request_repaint();
    }
    ctx.data_mut(|d| {
        d.insert_temp(mid, mem);
        d.insert_temp(
            id.with("motion/active"),
            Active { phase: mem.phase, elapsed, delay, count, next: 0 },
        );
    });
    ctx.data_mut(|d| d.insert_temp(Id::new("motion/current"), Some(id)));
    mem.shown
}

/// Ends the pass: remembers how many rows took part.
pub fn end(ctx: &Context, id: Id) {
    if !enabled() {
        return;
    }
    let active: Option<Active> = ctx.data(|d| d.get_temp(id.with("motion/active")));
    let mid = id.with("motion");
    if let (Some(a), Some(mut mem)) = (active, ctx.data(|d| d.get_temp::<Mem>(mid))) {
        mem.count = a.next;
        ctx.data_mut(|d| d.insert_temp(mid, mem));
    }
    ctx.data_mut(|d| d.insert_temp(Id::new("motion/current"), None::<Id>));
}

/// The opacity of the next row in the pass in progress (1 outside one).
pub fn next_alpha(ctx: &Context) -> f32 {
    if !enabled() {
        return 1.0;
    }
    let Some(id) = ctx.data(|d| d.get_temp::<Option<Id>>(Id::new("motion/current"))).flatten() else {
        return 1.0;
    };
    let aid = id.with("motion/active");
    let Some(mut a) = ctx.data(|d| d.get_temp::<Active>(aid)) else {
        return 1.0;
    };
    let i = a.next;
    a.next += 1;
    ctx.data_mut(|d| d.insert_temp(aid, a));
    match a.phase {
        1 => {
            let k = a.count.saturating_sub(1).saturating_sub(i);
            1.0 - ease((a.elapsed - k as f32 * a.delay) / FADE_SECS)
        }
        2 => ease((a.elapsed - i as f32 * a.delay) / FADE_SECS),
        _ => 1.0,
    }
}

/// Paints one row of the pass in progress at its turn's opacity.
pub fn scoped<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    let a = next_alpha(ui.ctx());
    if a >= 1.0 {
        return add(ui);
    }
    ui.scope(|ui| {
        ui.multiply_opacity(a);
        add(ui)
    })
    .inner
}

/// A stable key from anything hashable.
pub fn key_of(value: impl std::hash::Hash) -> u64 {
    use std::hash::Hasher;
    let mut h = std::collections::hash_map::DefaultHasher::new();
    value.hash(&mut h);
    h.finish()
}
