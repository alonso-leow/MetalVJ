//! Model 18 — Puzzle. A board of tiles that only ever changes by something
//! being **pushed into it from one of the four edges**, and the one interesting
//! case: two insertions arriving at the same instant on lines that cross.
//!
//! # The single mechanism
//!
//! Every insertion — plain or collided — is the same operation:
//!
//! ```text
//! open(row, col, arms) :  the board opens at one cell and the lines through
//!                         it shift outward along `arms`; the new piece takes
//!                         the cell that opened.
//! ```
//!
//! | what happened | opens at | arms |
//! |---|---|---|
//! | one piece from the left, into row r | `(r, 0)` | → |
//! | one from the right | `(r, cols-1)` | ← |
//! | one from the top, into column c | `(0, c)` | ↓ |
//! | one from the bottom | `(rows-1, c)` | ↑ |
//! | **two into the same row**, left and right | `(r, cols/2)` | ← → |
//! | **two into the same column**, top and bottom | `(rows/2, c)` | ↑ ↓ |
//! | **one row and one column** | `(r, c)`, where they cross | one each |
//!
//! The three bottom rows are the ask's *overlaps*: the two pieces never
//! arrive separately and then merge, because there is no instant at which the
//! board holds both. They arrive **as one piece**, at the cell their two lines
//! share, and the board pushes away from it in both directions at once. A
//! collision is therefore not a special case in the simulation — it is an
//! `open` with two arms instead of one.
//!
//! The two arrangements the ask says need no collision fall out of the same
//! rule and are not coded for: two or more pushes on *different* columns and no
//! rows, or on different rows and no columns, write disjoint sets of cells.
//! `verticales` at 0 or 1 is how an operator asks for exactly that.
//!
//! **A two-armed open duplicates the piece that was at the crossing.** Both
//! arms read it as their source, so it appears in both. That is the honest
//! bookkeeping rather than a bug to hide: the board gained one piece (the
//! collided one) and two ends each lost one, and the alternative — leaving one
//! arm's first slot empty — punches a hole in a full board.
//!
//! # Colour
//!
//! *"two colour codes for each piece insertion, to make gradients"*.
//! Each insertion is given two colours, sampled out of the `piezas` slot, and
//! the tile is a gradient between them along the direction it travelled. A
//! collided piece keeps **all four** — its own two and the other's — and is
//! drawn as a four-stop gradient across the diagonal its two arms make. That is
//! why the shader takes its stops per instance instead of from a `ColorSlot`
//! uniform: two colours and four colours have to be one code path.
//!
//! # Why the whole run is recomputed every frame
//!
//! A frame is a pure function of `(floats, time)`, like every other model here
//! except Turing — which is the one that had to grow a `SEEK_GAP` because
//! automix parks a model for thirty seconds and it came back re-seeding a whole
//! labyrinth. Puzzle has no such problem: the board `history()` steps ago is
//! irrelevant to the board now, because a line only holds `cols` (or `rows`)
//! pieces and is always fed from the same end, so enough pushes have flushed
//! it. The simulation runs that window on every frame and is exact.
//!
//! Two things had to be true for that to hold, and neither was true first time:
//!
//! - **A line is fed from one end, not from a hashed one per step.** Pushing a
//!   row right and then left is a random walk, and a row under a random walk
//!   never reliably fills — a 16x12 board still had a hole in it after seven
//!   hundred steps. [`flows_forward`] is the fix and it is also the better
//!   picture: rows and columns each keep a direction, so the board flows.
//! - **The window starts from a full board, not an empty one.** The window's
//!   start moves every frame, so anything derived from it would change under a
//!   tile nobody had overwritten yet; [`Board::seeded`] is a fixed hash of the
//!   operator's own palette, which cannot.

use wgpu::util::DeviceExt;

use crate::model::{color_at, background, spec, spec_held, Block, Built, Ctx, FloatSpec, Model, Schema};
use crate::shared::backdrop::Backdrop;
use crate::shared::ColorSlotGpu;

const MAX_COLS: usize = 16;
const MAX_ROWS: usize = 12;
/// Simultaneous insertions the model will ever schedule in one step.
const MAX_AT_ONCE: usize = 4;

const FLOATS: &[FloatSpec] = &[
    // The board's shape. Structural in the strongest sense in this suite: the
    // grid is not an intensity, and a beat that re-dealt 9x6 into 13x11 would
    // throw the whole board away four times a bar. Held near where the operator
    // put it, free to any slider.
    spec_held("columns", 4.0, MAX_COLS as f32, 9.0, 0.12),
    spec_held("rows", 3.0, MAX_ROWS as f32, 6.0, 0.12),
    // Insertions per second. The one float the music should own outright.
    spec("step", 0.15, 8.0, 1.6),
    // How many insertions a step may fire at once. Also structural — at 1 there
    // are never collisions at all, which is a different model, not a quieter one.
    spec_held("simultaneous", 1.0, MAX_AT_ONCE as f32, 2.0, 0.4),
    // How often a step with room for two is *arranged* so they collide.
    spec("overlay", 0.0, 1.0, 0.55),
    // The share of insertions that come from the top and bottom edges rather
    // than the sides. 0 and 1 are the ask's two collision-free arrangements.
    spec("vertical", 0.0, 1.0, 0.5),
    spec("separation", 0.0, 0.45, 0.12),
    spec("roundness", 0.0, 0.5, 0.22),
    spec("bevel", 0.0, 1.0, 0.45),
    // What fraction of a step is spent sliding. At 0 the board snaps, which is
    // a legitimate look and the reason this is a float and not a constant.
    spec("slide", 0.0, 1.0, 0.7),
];

/// Not white. Four white stops would make every piece the same white tile on a
/// white tile, which passes every render gate and shows nothing — the trap
/// `models::fractals` recorded for its own slot. The suite's greys and red.
const PIECES: [f32; 12] = [
    0.07, 0.07, 0.08, //
    0.55, 0.10, 0.10, //
    0.95, 0.35, 0.18, //
    0.97, 0.95, 0.91,
];

pub static SCHEMA: Schema = Schema {
    blocks: &[Block::Floats(FLOATS), color_at("pieces", &PIECES), background(), crate::model::SIGNAL_FLOATS_BLOCK, crate::model::EFFECTS_FLOATS_BLOCK, crate::model::EFFECTS_COLOR_BLOCK],
};

// ---------------------------------------------------------------------------
// the board
// ---------------------------------------------------------------------------

/// Which way a line shifts when a cell opens on it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Arm {
    Right,
    Left,
    Down,
    Up,
}

impl Arm {
    /// The step, in (column, row) cells. Row grows downward.
    fn delta(self) -> (i32, i32) {
        match self {
            Arm::Right => (1, 0),
            Arm::Left => (-1, 0),
            Arm::Down => (0, 1),
            Arm::Up => (0, -1),
        }
    }
}

#[derive(Clone, Copy)]
struct Cell {
    filled: bool,
    /// The four gradient stops, already resolved out of the `piezas` slot.
    stops: [[f32; 3]; 4],
    /// Which way the gradient runs across the tile.
    axis: [f32; 2],
    /// Where this cell's content was one step ago, relative to where it is
    /// now, in cells. `(0, 0)` is "it did not move".
    from: (f32, f32),
    /// Placed by a two-armed open. Those pop rather than slide, because there
    /// is no off-board cell for them to have come from.
    popped: bool,
    /// Still one of the tiles the board opened with, rather than one that has
    /// been pushed in. Nothing draws differently for it — it is here so
    /// `no_tile_on_screen_is_still_one_the_window_started_with` can check the
    /// one property that keeps the sliding window honest. See `simulate`.
    #[cfg_attr(not(test), allow(dead_code))]
    seeded: bool,
}

impl Default for Cell {
    fn default() -> Cell {
        Cell {
            filled: false,
            stops: [[0.0; 3]; 4],
            axis: [1.0, 0.0],
            from: (0.0, 0.0),
            popped: false,
            seeded: false,
        }
    }
}

/// A piece that left the board on the step being drawn, kept only so it can be
/// seen sliding out instead of vanishing at the edge.
struct Leaving {
    cell: Cell,
    /// Fractional board coordinates, so one cell past the edge is `-1` or `cols`.
    at: (f32, f32),
}

struct Board {
    cols: usize,
    rows: usize,
    cells: Vec<Cell>,
    leaving: Vec<Leaving>,
}

impl Board {
    fn new(cols: usize, rows: usize) -> Board {
        Board { cols, rows, cells: vec![Cell::default(); cols * rows], leaving: Vec::new() }
    }

    /// A board that is already full, out of the operator's own palette.
    ///
    /// The simulation window has to start *somewhere*, and starting it empty
    /// meant the board it produced was only full if every line happened to
    /// have been pushed more times than it is long. It usually was. When it was
    /// not, the frame had a hole in it — and a hole is the one artefact that
    /// makes a sliding puzzle read as broken rather than as sparse.
    ///
    /// Seeded from a fixed hash rather than from the window's start step, which
    /// matters: the window's start moves every frame, so a board seeded off it
    /// would change under any tile that had not been overwritten yet.
    fn seeded(cols: usize, rows: usize, slot: &ColorSlotGpu) -> Board {
        let mut board = Board::new(cols, rows);
        for i in 0..cols * rows {
            let key = i as u32;
            let a = slot.grad4(hash(key ^ 0x00A1_5EED));
            let b = slot.grad4(hash(key ^ 0x00B2_5EED));
            let angle = hash(key ^ 0x00C3_5EED) * std::f32::consts::TAU;
            board.cells[i] = Cell {
                filled: true,
                stops: two_stops(a, b),
                axis: [angle.cos(), angle.sin()],
                from: (0.0, 0.0),
                popped: false,
                seeded: true,
            };
        }
        board
    }

    fn at(&self, c: usize, r: usize) -> Cell {
        self.cells[r * self.cols + c]
    }

    fn put(&mut self, c: usize, r: usize, cell: Cell) {
        let i = r * self.cols + c;
        self.cells[i] = cell;
    }

    /// Forgets what the previous step did, so `from` describes this step only.
    fn settle(&mut self) {
        for cell in &mut self.cells {
            cell.from = (0.0, 0.0);
            cell.popped = false;
        }
        self.leaving.clear();
    }

    /// The board opens at `(c, r)` and the lines through it shift outward along
    /// every arm; `piece` takes the cell that opened.
    ///
    /// Two arms is a collision. Both read the old content of `(c, r)` as their
    /// source, which is what duplicates it — see the module header.
    fn open(&mut self, c: usize, r: usize, arms: Arms, piece: Cell) {
        for arm in arms.each() {
            let (dc, dr) = arm.delta();
            let source = (-dc as f32, -dr as f32);
            match arm {
                Arm::Right => {
                    let dropped = self.at(self.cols - 1, r);
                    if dropped.filled {
                        self.leaving.push(Leaving {
                            cell: Cell { from: source, popped: false, ..dropped },
                            at: (self.cols as f32, r as f32),
                        });
                    }
                    // Descending, so each receiver reads a slot not yet written.
                    for x in ((c + 1)..self.cols).rev() {
                        let mut moved = self.at(x - 1, r);
                        moved.from = source;
                        moved.popped = false;
                        self.put(x, r, moved);
                    }
                }
                Arm::Left => {
                    let dropped = self.at(0, r);
                    if dropped.filled {
                        self.leaving.push(Leaving {
                            cell: Cell { from: source, popped: false, ..dropped },
                            at: (-1.0, r as f32),
                        });
                    }
                    for x in 0..c {
                        let mut moved = self.at(x + 1, r);
                        moved.from = source;
                        moved.popped = false;
                        self.put(x, r, moved);
                    }
                }
                Arm::Down => {
                    let dropped = self.at(c, self.rows - 1);
                    if dropped.filled {
                        self.leaving.push(Leaving {
                            cell: Cell { from: source, popped: false, ..dropped },
                            at: (c as f32, self.rows as f32),
                        });
                    }
                    for y in ((r + 1)..self.rows).rev() {
                        let mut moved = self.at(c, y - 1);
                        moved.from = source;
                        moved.popped = false;
                        self.put(c, y, moved);
                    }
                }
                Arm::Up => {
                    let dropped = self.at(c, 0);
                    if dropped.filled {
                        self.leaving.push(Leaving {
                            cell: Cell { from: source, popped: false, ..dropped },
                            at: (c as f32, -1.0),
                        });
                    }
                    for y in 0..r {
                        let mut moved = self.at(c, y + 1);
                        moved.from = source;
                        moved.popped = false;
                        self.put(c, y, moved);
                    }
                }
            }
        }
        self.put(c, r, piece);
    }
}

// ---------------------------------------------------------------------------
// the schedule
// ---------------------------------------------------------------------------

/// Deterministic 32-bit hash to 0..1 — the same one `metal-dash::digest` uses, so
/// "random" here means the same thing it means everywhere else in the project.
fn hash(x: u32) -> f32 {
    let mut h = x.wrapping_add(0x9E37_79B9);
    h ^= h >> 16;
    h = h.wrapping_mul(0x85EB_CA6B);
    h ^= h >> 13;
    h = h.wrapping_mul(0xC2B2_AE35);
    h ^= h >> 16;
    (h >> 8) as f32 / 16_777_216.0
}

/// `hash` keyed on a step and a channel, so two draws from the same step never
/// collide by accident.
fn h2(step: u32, channel: u32) -> f32 {
    hash(step.wrapping_mul(0x9E37_79B9) ^ channel.wrapping_mul(0x85EB_CA6B).wrapping_add(1))
}

fn pick(step: u32, channel: u32, n: usize) -> usize {
    ((h2(step, channel) * n as f32) as usize).min(n.saturating_sub(1))
}

/// One arm, or two. Never more, and a fixed pair rather than a `Vec` because
/// `simulate` builds several hundred of these per frame and an allocation per
/// insertion is the whole cost of the model.
#[derive(Clone, Copy)]
struct Arms {
    a: Arm,
    b: Option<Arm>,
}

impl Arms {
    fn one(a: Arm) -> Arms {
        Arms { a, b: None }
    }

    fn two(a: Arm, b: Arm) -> Arms {
        Arms { a, b: Some(b) }
    }

    /// Two arms is a collision, one is a plain insertion — which is the whole
    /// of what the tests below need to tell them apart. Nothing in the model
    /// itself branches on the count, so this is only ever compiled for them.
    #[cfg(test)]
    fn len(self) -> usize {
        1 + usize::from(self.b.is_some())
    }

    fn each(self) -> impl Iterator<Item = Arm> {
        std::iter::once(self.a).chain(self.b)
    }
}

struct Insertion {
    c: usize,
    r: usize,
    arms: Arms,
    piece: Cell,
}

/// Two colours out of the slot become the four stops of a plain tile: the ends
/// as given, the middles interpolated. A collided tile skips this and keeps
/// four real colours, which is the only difference between the two.
fn two_stops(a: [f32; 3], b: [f32; 3]) -> [[f32; 3]; 4] {
    let mid = |k: f32| [a[0] + (b[0] - a[0]) * k, a[1] + (b[1] - a[1]) * k, a[2] + (b[2] - a[2]) * k];
    [a, mid(1.0 / 3.0), mid(2.0 / 3.0), b]
}

fn axis_of(arms: Arms) -> [f32; 2] {
    let mut x = 0.0;
    let mut y = 0.0;
    for arm in arms.each() {
        let (dc, dr) = arm.delta();
        x += dc as f32;
        // The tile's own space has y up; a row index grows downward.
        y -= dr as f32;
    }
    if x == 0.0 && y == 0.0 {
        return [1.0, 0.0];
    }
    let len = (x * x + y * y).sqrt();
    [x / len, y / len]
}

/// What arrives on step `k`.
fn schedule(
    k: u32,
    cols: usize,
    rows: usize,
    at_once: usize,
    fuse_bias: f32,
    vertical_bias: f32,
    slot: &ColorSlotGpu,
) -> Vec<Insertion> {
    // One colour out of the operator's palette, at a hashed point along it.
    let hue = |ch: u32| slot.grad4(h2(k, 900 + ch));

    // Whether this step is arranged to collide is decided *before* how many
    // insertions it has, and then it takes at least two. The other order — pick
    // a count, collide if it happens to be two or more — made `superposicion`
    // at 1 still mean "about half the steps", because half of them had drawn a
    // count of one and there was nothing for the second piece to hit.
    let fusing = at_once >= 2 && h2(k, 2) < fuse_bias;
    let mut n = 1 + (h2(k, 1) * at_once as f32) as usize;
    n = n.clamp(if fusing { 2 } else { 1 }, at_once);

    let mut out: Vec<Insertion> = Vec::with_capacity(n);
    let mut used_rows: Vec<usize> = Vec::new();
    let mut used_cols: Vec<usize> = Vec::new();

    if fusing {
        // A collision. Three ways two lines can share a cell, and all three are
        // in the ask by name.
        let (c, r, arms) = match pick(k, 3, 3) {
            0 => {
                // Same row, both ends. The row opens at its middle.
                let r = pick(k, 4, rows);
                (cols / 2, r, Arms::two(Arm::Left, Arm::Right))
            }
            1 => {
                // Same column, both ends.
                let c = pick(k, 5, cols);
                (c, rows / 2, Arms::two(Arm::Up, Arm::Down))
            }
            _ => {
                // One row and one column, at the cell where they cross.
                let r = pick(k, 6, rows);
                let c = pick(k, 7, cols);
                let row_arm = if h2(k, 8) < 0.5 { Arm::Right } else { Arm::Left };
                let col_arm = if h2(k, 9) < 0.5 { Arm::Down } else { Arm::Up };
                (c, r, Arms::two(row_arm, col_arm))
            }
        };
        used_rows.push(r);
        used_cols.push(c);
        out.push(Insertion {
            c,
            r,
            arms,
            // All four colours survive: two from each insertion. This is the
            // *"4-colour gradient"*.
            piece: Cell {
                filled: true,
                stops: [hue(0), hue(1), hue(2), hue(3)],
                axis: axis_of(arms),
                from: (0.0, 0.0),
                popped: true,
                seeded: false,
            },
        });
    }

    // Whatever is left over goes on lines nothing else is touching, so the only
    // collision in a step is the one that was arranged.
    let vertical = h2(k, 10) < vertical_bias;
    let plain = |arms: Arms, ch: u32| Cell {
        filled: true,
        stops: two_stops(hue(ch), hue(ch + 1)),
        axis: axis_of(arms),
        from: {
            let (dc, dr) = arms.a.delta();
            (-dc as f32, -dr as f32)
        },
        popped: false,
        seeded: false,
    };
    // Bounded rather than "keep hashing until a free line turns up": a board
    // narrower than the number of simultaneous insertions has to end the step
    // short instead of spinning.
    let mut ch = 20u32;
    for _ in 0..(MAX_AT_ONCE * 8) {
        if out.len() >= n {
            break;
        }
        ch += 3;
        if vertical {
            if used_cols.len() >= cols {
                break;
            }
            let c = pick(k, ch, cols);
            if used_cols.contains(&c) {
                continue;
            }
            used_cols.push(c);
            let arms = Arms::one(if flows_forward(c as u32, 0x0C01) { Arm::Down } else { Arm::Up });
            let r = if arms.a == Arm::Down { 0 } else { rows - 1 };
            out.push(Insertion { c, r, arms, piece: plain(arms, ch) });
        } else {
            if used_rows.len() >= rows {
                break;
            }
            let r = pick(k, ch, rows);
            if used_rows.contains(&r) {
                continue;
            }
            used_rows.push(r);
            let arms =
                Arms::one(if flows_forward(r as u32, 0x0404) { Arm::Right } else { Arm::Left });
            let c = if arms.a == Arm::Right { 0 } else { cols - 1 };
            out.push(Insertion { c, r, arms, piece: plain(arms, ch) });
        }
    }
    out
}

/// Which way a given line is fed from — a property of the **line**, not of the
/// step.
///
/// It used to be hashed per step, so a row was pushed right, then left, then
/// right. That is a random walk, and a row under one is not a conveyor: pieces
/// fed in from the left are carried back out of the left by the next push, and
/// a line can sit half empty for a long time. Rows and columns each keeping
/// their own direction is also what makes the board read as *moving* rather
/// than as shuffling.
///
/// Collisions still come from two sides at once — that is what a collision is,
/// and it is the one place the rule does not apply.
fn flows_forward(line: u32, salt: u32) -> bool {
    hash(line ^ salt) < 0.5
}

/// How many steps back the simulation starts.
///
/// A line only holds `cols` (or `rows`) pieces and is always fed from the same
/// end (see [`flows_forward`]), so a line pushed more times than it is long has
/// nothing left in it from before the window. This is that many times over, for
/// the line that is picked least.
fn history(cols: usize, rows: usize) -> u32 {
    (36 * (cols + rows) as u32).min(1200)
}

fn simulate(
    k: u32,
    cols: usize,
    rows: usize,
    at_once: usize,
    fuse_bias: f32,
    vertical_bias: f32,
    slot: &ColorSlotGpu,
) -> Board {
    let mut board = Board::seeded(cols, rows, slot);
    let first = k.saturating_sub(history(cols, rows));
    for step in first..=k {
        board.settle();
        for ins in schedule(step, cols, rows, at_once, fuse_bias, vertical_bias, slot) {
            board.open(ins.c, ins.r, ins.arms, ins.piece);
        }
    }
    board
}

// ---------------------------------------------------------------------------
// the GPU side
// ---------------------------------------------------------------------------

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniform {
    misc: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Tile {
    rect: [f32; 4],
    axis: [f32; 4],
    c0: [f32; 4],
    c1: [f32; 4],
    c2: [f32; 4],
    c3: [f32; 4],
}

struct Gpu {
    pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    backdrop: Backdrop,
}

pub struct Puzzle {
    gpu: Built<Gpu>,
}

impl Puzzle {
    pub fn new() -> Puzzle {
        Puzzle { gpu: Built::default() }
    }

    fn build(device: &wgpu::Device, format: wgpu::TextureFormat) -> Gpu {
        let shader = crate::shared::build_shader(
            device,
            "puzzle",
            include_str!("../../shaders/puzzle.wgsl"),
        );

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("puzzle bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("puzzle layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });

        let instance_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Tile>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &wgpu::vertex_attr_array![
                0 => Float32x4, 1 => Float32x4, 2 => Float32x4,
                3 => Float32x4, 4 => Float32x4, 5 => Float32x4
            ],
        };

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("puzzle pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[Some(instance_layout)],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        Gpu { pipeline, bind_group_layout, backdrop: Backdrop::new(device, format) }
    }
}

/// The board as tiles, in the aspect-corrected space the shader draws in.
///
/// Split out of `render` so the whole geometry can be checked without a GPU —
/// "does a collision put one tile where two insertions met" is a question about
/// this function and not about wgpu.
fn tiles(floats: &[f32], time: f32, aspect: f32) -> Vec<Tile> {
    let cols = (floats[0].round() as usize).clamp(4, MAX_COLS);
    let rows = (floats[1].round() as usize).clamp(3, MAX_ROWS);
    let rate = floats[2].max(0.01);
    let at_once = (floats[3].round() as usize).clamp(1, MAX_AT_ONCE);
    let fuse_bias = floats[4].clamp(0.0, 1.0);
    let vertical_bias = floats[5].clamp(0.0, 1.0);
    let separation = floats[6].clamp(0.0, 0.45);
    let roundness = floats[7].clamp(0.0, 0.5);
    let slide = floats[9].clamp(0.0, 1.0);
    let slot = ColorSlotGpu::from_floats(&floats[10..22]);

    let clock = (time.max(0.0) * rate) as f64;
    let k = clock.floor().clamp(0.0, 2.0e9) as u32;
    let frac = (clock - clock.floor()) as f32;

    let board = simulate(k, cols, rows, at_once, fuse_bias, vertical_bias, &slot);

    // How far through the slide this frame is. `deslizamiento` is what fraction
    // of the step the movement occupies; the rest of it the board sits still.
    let raw = if slide <= 1e-3 { 1.0 } else { (frac / slide).clamp(0.0, 1.0) };
    let ease = raw * raw * (3.0 - 2.0 * raw);

    let cw = 2.0 * aspect / cols as f32;
    let chh = 2.0 / rows as f32;
    let half = [cw * 0.5 * (1.0 - separation), chh * 0.5 * (1.0 - separation)];
    let radius = half[0].min(half[1]) * roundness * 2.0;

    let mut out = Vec::with_capacity(cols * rows + 8);
    let mut push = |cell: &Cell, fc: f32, fr: f32| {
        // Where it is now, plus however much of its move is still to happen.
        let x = -aspect + (fc + 0.5) * cw + cell.from.0 * cw * (1.0 - ease);
        let y = 1.0 - (fr + 0.5) * chh - cell.from.1 * chh * (1.0 - ease);
        // A collided piece has no off-board cell to slide in from, so it pops
        // instead — which is also the only cue that a collision just happened.
        let scale = if cell.popped {
            let e = ease;
            0.15 + 0.85 * (1.0 - (1.0 - e) * (1.0 - e) * (1.0 - e)) * (1.0 + 0.18 * (1.0 - e))
        } else {
            1.0
        };
        out.push(Tile {
            rect: [x, y, half[0] * scale, half[1] * scale],
            axis: [cell.axis[0], cell.axis[1], 1.0, radius * scale],
            c0: [cell.stops[0][0], cell.stops[0][1], cell.stops[0][2], 1.0],
            c1: [cell.stops[1][0], cell.stops[1][1], cell.stops[1][2], 1.0],
            c2: [cell.stops[2][0], cell.stops[2][1], cell.stops[2][2], 1.0],
            c3: [cell.stops[3][0], cell.stops[3][1], cell.stops[3][2], 1.0],
        });
    };

    // The ones on their way out first, so a board tile always covers them.
    for gone in &board.leaving {
        push(&gone.cell, gone.at.0, gone.at.1);
    }
    for r in 0..rows {
        for c in 0..cols {
            let cell = board.at(c, r);
            if cell.filled {
                push(&cell, c as f32, r as f32);
            }
        }
    }
    out
}

impl Model for Puzzle {
    fn id(&self) -> &'static str {
        "puzzle"
    }

    fn name(&self) -> &'static str {
        "Puzzle"
    }

    fn schema(&self) -> &'static Schema {
        &SCHEMA
    }

    fn backdrop(&self) -> bool {
        true
    }

    fn render(
        &mut self,
        ctx: &Ctx,
        floats: &[f32],
        target: &wgpu::TextureView,
        format: wgpu::TextureFormat,
    ) {
        let gpu = self.gpu.get(format, || Self::build(ctx.device, format));

        gpu.backdrop.draw(
            ctx,
            target,
            ColorSlotGpu::from_floats(&floats[22..34]),
            0.0,
            ctx.fondo_alpha,
        );

        let instances = tiles(floats, ctx.time, ctx.aspect());
        if instances.is_empty() {
            return;
        }

        let uniform = Uniform { misc: [ctx.aspect(), floats[8].clamp(0.0, 1.0), 0.0, 0.0] };
        let ubuf = ctx.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("puzzle uniform"),
            contents: bytemuck::bytes_of(&uniform),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let ibuf = ctx.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("puzzle instances"),
            contents: bytemuck::cast_slice(&instances),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("puzzle bind group"),
            layout: &gpu.bind_group_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: ubuf.as_entire_binding() }],
        });

        let mut encoder = ctx.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("puzzle encoder"),
        });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("puzzle pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    resolve_target: None,
                    depth_slice: None,
                    // The backdrop above already cleared to the `fondo` slot.
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&gpu.pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.set_vertex_buffer(0, ibuf.slice(..));
            pass.draw(0..4, 0..instances.len() as u32);
        }
        ctx.queue.submit(Some(encoder.finish()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row() -> Vec<f32> {
        SCHEMA.defaults()
    }

    fn slot() -> ColorSlotGpu {
        ColorSlotGpu::from_floats(&PIECES)
    }

    #[test]
    fn the_schema_and_the_row_the_renderer_reads_are_the_same_width() {
        // 10 floats + two colour slots. `render` indexes `floats[22..34]` by
        // hand, so a block added above it silently reads a colour out of the
        // wrong twelve.
        assert_eq!(SCHEMA.width(), 34 + crate::model::SIGNAL_WIDTH + crate::model::EFFECTS_WIDTH);
        assert_eq!(row().len(), 34 + crate::model::SIGNAL_WIDTH + crate::model::EFFECTS_WIDTH);
    }

    #[test]
    fn a_plain_insertion_lands_on_the_edge_it_came_from_and_pushes_one_cell() {
        let mut board = Board::new(6, 4);
        let mark = Cell { filled: true, stops: [[1.0, 0.0, 0.0]; 4], ..Cell::default() };
        // Seed a piece where the push should move it to.
        board.put(0, 1, Cell { filled: true, stops: [[0.0, 1.0, 0.0]; 4], ..Cell::default() });
        board.open(0, 1, Arms::one(Arm::Right), mark);
        assert!(board.at(0, 1).filled, "the new piece is not at the edge it entered");
        assert_eq!(board.at(0, 1).stops[0], [1.0, 0.0, 0.0]);
        assert_eq!(board.at(1, 1).stops[0], [0.0, 1.0, 0.0], "the row did not shift");
        assert_eq!(board.at(1, 1).from, (-1.0, 0.0), "the shift did not record where it came from");
    }

    #[test]
    fn a_push_off_the_far_end_leaves_the_board_and_is_kept_to_be_seen_going() {
        let mut board = Board::new(4, 3);
        for c in 0..4 {
            board.put(c, 0, Cell { filled: true, ..Cell::default() });
        }
        board.settle();
        board.open(0, 0, Arms::one(Arm::Right), Cell { filled: true, ..Cell::default() });
        assert_eq!(board.leaving.len(), 1, "the piece pushed off the end just vanished");
        assert_eq!(board.leaving[0].at, (4.0, 0.0), "it should be one cell past the right edge");
    }

    /// The ask's three collision cases, each of which has to put **one** piece
    /// where the two lines meet rather than two pieces on two edges.
    #[test]
    fn two_insertions_in_the_same_row_become_one_piece_in_the_middle_of_it() {
        let mut board = Board::new(7, 5);
        let fused = Cell { filled: true, stops: [[0.9, 0.1, 0.1]; 4], popped: true, ..Cell::default() };
        board.open(7 / 2, 2, Arms::two(Arm::Left, Arm::Right), fused);
        assert!(board.at(3, 2).filled);
        assert!(board.at(3, 2).popped, "a collided piece has nowhere to slide in from");
        // And it is the only new piece: neither edge of that row got one.
        assert!(!board.at(0, 2).filled);
        assert!(!board.at(6, 2).filled);
    }

    #[test]
    fn a_row_and_a_column_collide_at_the_cell_where_they_cross() {
        let mut board = Board::new(8, 6);
        let fused = Cell { filled: true, stops: [[0.2, 0.6, 0.9]; 4], popped: true, ..Cell::default() };
        board.open(5, 3, Arms::two(Arm::Right, Arm::Down), fused);
        assert_eq!(board.at(5, 3).stops[0], [0.2, 0.6, 0.9]);
        // Both arms pushed away from it, so the crossing's old content — empty
        // here — is what moved, and nothing was placed on either entry edge.
        assert!(!board.at(0, 3).filled, "a collision must not also insert at the row's edge");
        assert!(!board.at(5, 0).filled, "a collision must not also insert at the column's edge");
    }

    #[test]
    fn a_collided_piece_keeps_four_real_colours_and_a_plain_one_is_two() {
        // *"The colours in the overlaps are recalculated taking into account the
        // four colours that make them up"* — so the four stops of a collided
        // piece must not be the two-colour ramp a plain piece gets.
        let plain = two_stops([1.0, 0.0, 0.0], [0.0, 0.0, 1.0]);
        assert_eq!(plain[0], [1.0, 0.0, 0.0]);
        assert_eq!(plain[3], [0.0, 0.0, 1.0]);
        assert!((plain[1][0] - 2.0 / 3.0).abs() < 1e-5, "the middles are not on the ramp");

        let all = schedule(11, 9, 6, 4, 1.0, 0.5, &slot());
        let fused = all.iter().find(|i| i.arms.len() == 2).expect("a fusion was asked for");
        let s = fused.piece.stops;
        assert!(
            s[0] != s[1] || s[1] != s[2] || s[2] != s[3],
            "a four-colour gradient collapsed to one colour"
        );
    }

    #[test]
    fn every_step_that_asks_for_a_collision_schedules_exactly_one() {
        for k in 0..400u32 {
            let all = schedule(k, 11, 7, 4, 1.0, 0.5, &slot());
            let fusions = all.iter().filter(|i| i.arms.len() == 2).count();
            assert_eq!(fusions, 1, "step {k} scheduled {fusions} collisions");
        }
    }

    /// *"The cases of simultaneous insertion that need no overlap are when
    /// there are insertions in two or more different columns and no row, or
    /// in two or more different rows and no column."*
    #[test]
    fn with_collisions_off_a_step_only_ever_touches_lines_of_one_kind_and_never_twice() {
        for k in 0..400u32 {
            for vertical in [0.0f32, 1.0] {
                let all = schedule(k, 11, 7, 4, 0.0, vertical, &slot());
                assert!(all.iter().all(|i| i.arms.len() == 1), "step {k} collided with bias 0");
                let horizontal: Vec<_> =
                    all.iter().filter(|i| matches!(i.arms.a, Arm::Left | Arm::Right)).collect();
                let up_down: Vec<_> =
                    all.iter().filter(|i| matches!(i.arms.a, Arm::Up | Arm::Down)).collect();
                assert!(
                    horizontal.is_empty() || up_down.is_empty(),
                    "step {k} mixed rows and columns with collisions off"
                );
                let mut rows: Vec<usize> = horizontal.iter().map(|i| i.r).collect();
                rows.sort_unstable();
                let n = rows.len();
                rows.dedup();
                assert_eq!(rows.len(), n, "step {k} pushed the same row twice");
                let mut cols: Vec<usize> = up_down.iter().map(|i| i.c).collect();
                cols.sort_unstable();
                let n = cols.len();
                cols.dedup();
                assert_eq!(cols.len(), n, "step {k} pushed the same column twice");
            }
        }
    }

    #[test]
    fn the_board_never_has_a_hole_in_it() {
        // A hole is the one artefact that makes a sliding puzzle read as broken
        // rather than as sparse, and it is what an empty starting board gave
        // whenever a line was pushed fewer times than it is long.
        for (cols, rows) in [(4, 3), (9, 6), (16, 12)] {
            for k in [0u32, 1, 7, history(cols, rows) + 40] {
                let board = simulate(k, cols, rows, 2, 0.5, 0.5, &slot());
                let holes = board.cells.iter().filter(|c| !c.filled).count();
                assert_eq!(holes, 0, "{cols}x{rows} at step {k} had {holes} empty cells");
            }
        }
    }

    /// The property that keeps the sliding window honest, checked directly.
    ///
    /// The window starts `history()` steps back and moves forward one step per
    /// step. A tile still holding what the board opened with is a tile whose
    /// value depends on where the window happens to start — so the frame after
    /// this one, whose window starts one step later, could draw it differently
    /// for no reason an audience could account for. None may survive.
    #[test]
    fn no_tile_on_screen_is_still_one_the_window_started_with() {
        for (cols, rows) in [(4, 3), (9, 6), (16, 12)] {
            for vertical in [0.0f32, 0.5, 1.0] {
                let k = history(cols, rows) + 500;
                let board = simulate(k, cols, rows, 2, 0.5, vertical, &slot());
                let left = board.cells.iter().filter(|c| c.seeded).count();
                assert_eq!(
                    left, 0,
                    "{cols}x{rows} (verticales {vertical}) still had {left} of the opening tiles \
                     after a whole window"
                );
            }
        }
    }

    /// And the same, frame to frame: two consecutive steps must not disagree
    /// about a tile neither of them touched.
    #[test]
    fn the_window_moving_forward_does_not_change_the_past() {
        let k = history(9, 6) + 200;
        let a = simulate(k, 9, 6, 2, 0.5, 0.5, &slot());
        let b = simulate(k, 9, 6, 2, 0.5, 0.5, &slot());
        for (x, y) in a.cells.iter().zip(b.cells.iter()) {
            assert_eq!(x.stops[0], y.stops[0]);
        }
        // The step after it differs only where that step reached.
        let next = simulate(k + 1, 9, 6, 2, 0.5, 0.5, &slot());
        let touched: usize = schedule(k + 1, 9, 6, 2, 0.5, 0.5, &slot())
            .iter()
            .map(|i| if i.arms.len() == 2 { 9 + 6 } else { 9.max(6) })
            .sum();
        let changed = a
            .cells
            .iter()
            .zip(next.cells.iter())
            .filter(|(x, y)| x.stops[0] != y.stops[0])
            .count();
        assert!(
            changed <= touched,
            "one step changed {changed} tiles but could only reach {touched}"
        );
    }

    #[test]
    fn a_frame_is_a_pure_function_of_the_row_and_the_clock() {
        // Not a nicety: the preview and the monitor ask for the same instant
        // separately, and automix parks a model for half a minute and comes
        // back. Turing needed a `SEEK_GAP` for exactly this; Puzzle must not.
        let r = row();
        let a = tiles(&r, 41.37, 16.0 / 9.0);
        let b = tiles(&r, 41.37, 16.0 / 9.0);
        assert_eq!(a.len(), b.len());
        for (x, y) in a.iter().zip(b.iter()) {
            assert_eq!(x.rect, y.rect);
            assert_eq!(x.c0, y.c0);
        }
    }

    #[test]
    fn every_tile_is_inside_the_frame_or_on_its_way_out_of_it() {
        let r = row();
        let aspect = 16.0 / 9.0;
        let mut t = 0.0;
        while t < 30.0 {
            for tile in tiles(&r, t, aspect) {
                assert!(tile.rect[0].is_finite() && tile.rect[1].is_finite(), "a tile went non-finite");
                assert!(
                    tile.rect[0].abs() < aspect + 2.0 && tile.rect[1].abs() < 3.0,
                    "a tile at {:?} is nowhere near the frame",
                    tile.rect
                );
                assert!(tile.rect[2] > 0.0 && tile.rect[3] > 0.0, "a tile has no size");
            }
            t += 0.137;
        }
    }

    #[test]
    fn the_board_keeps_its_shape_at_both_ends_of_every_slider() {
        // Grids at their declared limits are where an off-by-one in `open`
        // shows up, and `columnas`/`filas` are the two floats a driven row can
        // still nudge.
        for (cols, rows) in [(4.0, 3.0), (16.0, 12.0), (16.0, 3.0), (4.0, 12.0)] {
            let mut r = row();
            r[0] = cols;
            r[1] = rows;
            for at_once in 1..=MAX_AT_ONCE {
                r[3] = at_once as f32;
                let drawn = tiles(&r, 12.5, 16.0 / 9.0);
                assert!(!drawn.is_empty(), "{cols}x{rows} x{at_once} drew nothing at all");
            }
        }
    }
}
