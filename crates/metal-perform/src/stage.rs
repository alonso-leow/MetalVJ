//! The venue → stage hitbox adapter: a real patch (`venue::Venue` + its fixture profiles)
//! becomes a [`metal_stage::Patch`] the AI's weights and the dataset speak in. It also
//! feeds the two read tools of the AI (`stage_grid`'s report and `stage_view`'s picture),
//! so the grid the chat reads is the grid the matrix is built from. It also turns a
//! stage-float row into one [`Signal`] per fixture (`expand_row`) and runs the trained
//! `ai.stage` weights of a patch on a music row (`ai_row`).

use crate::fixture;
use crate::venue::{FixtureKind, Venue};
use metal_schema::Signal;
use metal_stage::{Channel, Kind, Patch, PatchFixture, Point, StageModel};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// `Fixture::new` hangs every light at yaw 0 / pitch -20; a pose that is still that was
/// never set by hand, so the hitbox aim applies. Anything else is fine-tuned and wins.
fn tuned_pose(yaw: f32, pitch: f32) -> Option<(f32, f32)> {
    ((yaw, pitch) != (0.0, -20.0) && (yaw, pitch) != (0.0, 0.0)).then_some((yaw, pitch))
}

fn kind_of(k: FixtureKind) -> Option<Kind> {
    Kind::parse(k.word())
}

/// A stage point as the venue stores it: the grid's own word (`l2.back.l`), checked.
/// The empty string is not a point; clearing a pin is `null` or `""` at the route.
pub fn pin_word(s: &str) -> Result<String, String> {
    Point::parse(s).map(|p| p.word()).ok_or_else(|| {
        format!("unknown stage point {s:?}: a grid point such as l2.back.l (levels l1|l2|l3, rows back|mid|pro|house, columns l|c|r)")
    })
}

/// The stage with no venue in it: the default frame and no fixtures, what `stage_view`
/// draws when there is no venue to read yet.
pub fn empty_patch() -> Patch {
    Patch::new("none", Vec::new())
}

/// Every emitting fixture of the venue, with the channels its profile drives. A fixture
/// whose pose is not the 0/0 default counts as fine-tuned (the venue wins over the grid).
/// A pinned fixture (`stage_point`) is placed on that point instead of the nearest one.
pub fn patch_of(venue: &Venue, root: &Path) -> Patch {
    let profiles = fixture::load(root);
    let fixtures = venue
        .fixtures
        .iter()
        .filter_map(|f| {
            let kind = kind_of(f.kind)?;
            let channels = f
                .profile
                .as_deref()
                .and_then(|m| profiles.iter().find(|p| p.model == m))
                .map(|p| p.selectors().iter().filter_map(|s| Channel::parse(s.name())).collect())
                .unwrap_or_default();
            // A pinned spot (`stage_point`: a point or a line, optionally its variant, set
            // by the hitbox view or the venue file) wins over the nearest point.
            let spot = f.stage_point.as_deref().and_then(metal_stage::Spot::parse);
            Some(PatchFixture {
                id: f.id.clone(),
                kind,
                at: f.at?,
                group: f.group.clone(),
                channels,
                point: spot.map(|s| s.point),
                pose: tuned_pose(f.yaw, f.pitch),
                pixels: f.pixels.unwrap_or(0).min(u16::MAX as usize) as u16,
                variant: spot.and_then(|s| s.variant),
            })
        })
        .collect();
    Patch::new(&venue.venue.id, fixtures)
}

/// What `stage_grid` answers: the grid's points, the float names of the patch, where each
/// fixture stands (point, aim, pinned or not), its cells, and what the grid cannot fill.
/// Everything the AI needs to ask the operator the right question next.
pub fn grid_report(venue: &Venue, root: &Path) -> Value {
    let patch = patch_of(venue, root);
    let frame = patch.frame;
    let matrix = patch.matrix();
    let places = patch.placements();

    let points: Vec<Value> = Point::all()
        .into_iter()
        .map(|p| json!({ "point": p.word(), "at": frame.at(p) }))
        .collect();
    let placements: Vec<Value> = patch
        .fixtures
        .iter()
        .zip(&places)
        .map(|(f, p)| {
            json!({
                "fixture": p.fixture,
                "kind": f.kind.word(),
                "point": p.point.word(),
                "pinned": f.point.is_some(),
                "yaw": p.yaw,
                "pitch": p.pitch,
                "tuned": p.tuned,
                "group": f.group,
            })
        })
        .collect();
    let cells: Vec<Value> = patch
        .cells()
        .into_iter()
        .map(|((p, k), ids)| json!({ "point": p.word(), "kind": k.word(), "fixtures": ids }))
        .collect();

    // Unfilled: a venue fixture the grid cannot place or give floats to.
    let mut unfilled = Vec::new();
    for f in &venue.fixtures {
        if kind_of(f.kind).is_none() {
            unfilled.push(json!({
                "fixture": f.id,
                "reason": format!("a {} has no stage words: it emits no float", f.kind.word()),
            }));
        } else if f.at.is_none() {
            unfilled.push(json!({
                "fixture": f.id,
                "reason": "no position: `at` is not set, so no point is nearest",
            }));
        }
    }

    // Ambiguous: an unpinned fixture that stands between two points (the nearest two are
    // within 0.3 m). The AI asks where it hangs, or pins it with `stage_place`.
    let mut ambiguous = Vec::new();
    for (f, p) in patch.fixtures.iter().zip(&places) {
        if f.point.is_some() {
            continue;
        }
        let mut near: Vec<(f32, Point)> = Point::points()
            .into_iter()
            .map(|q| {
                let c = frame.at(q);
                let d = [c[0] - f.at[0], c[1] - f.at[1], c[2] - f.at[2]];
                (d[0] * d[0] + d[1] * d[1] + d[2] * d[2], q)
            })
            .collect();
        near.sort_by(|a, b| a.0.total_cmp(&b.0));
        if near[1].0.sqrt() - near[0].0.sqrt() < 0.3 {
            ambiguous.push(json!({
                "fixture": f.id,
                "points": [p.point.word(), near[1].1.word()],
                "reason": "between two points: ask where it hangs, or pin it with stage_place",
            }));
        }
    }

    json!({
        "venue": venue.venue.id,
        "format": "<level>.<row>.<col>.<type>.<channel>, e.g. l2.back.l.wash.level",
        "frame": frame,
        "points": points,
        "floats": matrix.names(),
        "placements": placements,
        "cells": cells,
        "unfilled": unfilled,
        "ambiguous": ambiguous,
    })
}

/// A plain RGB canvas: the picture `stage_view` draws the grid and the rig on.
struct Canvas {
    w: u32,
    h: u32,
    px: Vec<[u8; 3]>,
}

impl Canvas {
    fn new(w: u32, h: u32, bg: [u8; 3]) -> Canvas {
        Canvas { w, h, px: vec![bg; (w * h) as usize] }
    }
    fn put(&mut self, x: i32, y: i32, c: [u8; 3]) {
        if x >= 0 && y >= 0 && (x as u32) < self.w && (y as u32) < self.h {
            self.px[(y as u32 * self.w + x as u32) as usize] = c;
        }
    }
    fn disc(&mut self, cx: f32, cy: f32, r: f32, c: [u8; 3]) {
        let r2 = r * r;
        for dy in -(r.ceil() as i32)..=(r.ceil() as i32) {
            for dx in -(r.ceil() as i32)..=(r.ceil() as i32) {
                if (dx * dx + dy * dy) as f32 <= r2 {
                    self.put(cx.round() as i32 + dx, cy.round() as i32 + dy, c);
                }
            }
        }
    }
    fn ring(&mut self, cx: f32, cy: f32, r: f32, c: [u8; 3]) {
        for dy in -(r.ceil() as i32 + 1)..=(r.ceil() as i32 + 1) {
            for dx in -(r.ceil() as i32 + 1)..=(r.ceil() as i32 + 1) {
                let d2 = (dx * dx + dy * dy) as f32;
                if d2 <= r * r && d2 >= (r - 1.2) * (r - 1.2) {
                    self.put(cx.round() as i32 + dx, cy.round() as i32 + dy, c);
                }
            }
        }
    }
    fn line(&mut self, a: (f32, f32), b: (f32, f32), c: [u8; 3]) {
        let steps = (b.0 - a.0).abs().max((b.1 - a.1).abs()).ceil().max(1.0) as i32;
        for i in 0..=steps {
            let t = i as f32 / steps as f32;
            self.put((a.0 + (b.0 - a.0) * t).round() as i32, (a.1 + (b.1 - a.1) * t).round() as i32, c);
        }
    }
    fn fill(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, c: [u8; 3]) {
        for y in y0.round() as i32..=y1.round() as i32 {
            for x in x0.round() as i32..=x1.round() as i32 {
                self.put(x, y, c);
            }
        }
    }
}

/// The colour a fixture kind is drawn in on the plan.
fn kind_colour(k: Kind) -> [u8; 3] {
    match k {
        Kind::Wash => [255, 150, 60],
        Kind::Spot => [250, 230, 90],
        Kind::Moving => [230, 90, 220],
        Kind::Backlight => [90, 120, 255],
        Kind::Panel => [120, 230, 120],
        Kind::Continuous => [240, 240, 240],
        Kind::LedBar => [70, 220, 200],
        Kind::LedStrip => [160, 255, 90],
        Kind::Matrix => [255, 90, 90],
    }
}

/// The stage's plan (looking down, the audience at the bottom): the stage floor, the
/// proscenium, the house line, the 24 grid points (floor grey, rig cyan, zenith ring),
/// and each fixture at its real position in its kind's colour, a white ring when pinned,
/// and a tick along its aim. An empty rig draws the grid alone. Works in any venue state.
pub fn plan_png(patch: &Patch, w: u32, h: u32) -> Result<Vec<u8>, String> {
    let frame = patch.frame;
    let points: Vec<[f32; 3]> = Point::points().into_iter().map(|p| frame.at(p)).collect();
    let mut xs: Vec<f32> = points.iter().map(|p| p[0]).collect();
    let mut zs: Vec<f32> = points.iter().map(|p| p[2]).collect();
    for f in &patch.fixtures {
        xs.push(f.at[0]);
        zs.push(f.at[2]);
    }
    let (xmin, xmax) = (xs.iter().cloned().fold(f32::MAX, f32::min) - 1.0, xs.iter().cloned().fold(f32::MIN, f32::max) + 1.0);
    let (zmin, zmax) = (zs.iter().cloned().fold(f32::MAX, f32::min) - 1.0, zs.iter().cloned().fold(f32::MIN, f32::max) + 1.0);
    let margin = 16.0;
    let scale = ((w as f32 - 2.0 * margin) / (xmax - xmin)).min((h as f32 - 2.0 * margin) / (zmax - zmin));
    let ox = (w as f32 - (xmax - xmin) * scale) / 2.0;
    let oy = (h as f32 - (zmax - zmin) * scale) / 2.0;
    let to_px = |x: f32, z: f32| -> (f32, f32) { (ox + (x - xmin) * scale, oy + (z - zmin) * scale) };

    let mut c = Canvas::new(w, h, [16, 20, 26]);
    // The stage floor and the proscenium (z = 0), the house line.
    let (a, b) = (to_px(-frame.width / 2.0, -frame.depth), to_px(frame.width / 2.0, 0.0));
    c.fill(a.0, a.1, b.0, b.1, [30, 36, 46]);
    let (l, r) = (to_px(-frame.width / 2.0, 0.0), to_px(frame.width / 2.0, 0.0));
    c.line(l, r, [210, 210, 210]);
    let (hl, hr) = (to_px(xmin, frame.house), to_px(xmax, frame.house));
    c.line(hl, hr, [90, 90, 110]);

    for (p, at) in Point::points().into_iter().zip(&points) {
        let q = to_px(at[0], at[2]);
        match p.level {
            metal_stage::Level::Floor => c.disc(q.0, q.1, 2.0, [150, 150, 150]),
            metal_stage::Level::Rig => c.disc(q.0, q.1, 2.0, [80, 200, 230]),
            metal_stage::Level::Zenith => c.ring(q.0, q.1, 3.0, [240, 220, 90]),
        }
    }

    for (f, p) in patch.fixtures.iter().zip(patch.placements()) {
        let q = to_px(f.at[0], f.at[2]);
        let colour = kind_colour(f.kind);
        if f.point.is_some() {
            c.ring(q.0, q.1, 7.0, [255, 255, 255]);
        }
        c.disc(q.0, q.1, 4.0, colour);
        // Aim: yaw 0 looks at the back wall (-z), +90 looks +x.
        let yaw = p.yaw.to_radians();
        let tip = (q.0 + yaw.sin() * 14.0, q.1 - yaw.cos() * 14.0);
        c.line(q, tip, colour);
    }

    encode_rgb_png(&c)
}

fn encode_rgb_png(c: &Canvas) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, c.w, c.h);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        let mut writer = enc.write_header().map_err(|e| e.to_string())?;
        let flat: Vec<u8> = c.px.iter().flat_map(|p| *p).collect();
        writer.write_image_data(&flat).map_err(|e| e.to_string())?;
    }
    Ok(out)
}

/// Where a patch's trained `ai.stage` weights live: `<root>/stage/<patch>.json`.
pub fn model_path(root: &Path, patch: &str) -> PathBuf {
    root.join("stage").join(format!("{patch}.json"))
}

/// The patch's trained weights, or `None` when there are none, the file does not parse, or
/// it was fitted for another patch ([`StageModel::fits`]: name, hash and columns).
pub fn load_model(root: &Path, patch: &Patch) -> Option<StageModel> {
    let text = metal_vfs::read_to_string(model_path(root, &patch.name)).ok()?;
    serde_json::from_str::<StageModel>(&text).ok().filter(|m| m.fits(patch))
}

/// Writes the weights beside the patch name; returns the file written.
pub fn save_model(root: &Path, model: &StageModel) -> Result<PathBuf, String> {
    let path = model_path(root, &model.patch);
    if let Some(dir) = path.parent() {
        metal_vfs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let text = serde_json::to_string_pretty(model).map_err(|e| e.to_string())?;
    metal_vfs::write(&path, text).map_err(|e| e.to_string())?;
    Ok(path)
}

/// Trains the venue's `ai.stage` weights from the authored examples under
/// `<root>/dataset/stage/examples` and saves them. Only approved, valid examples cut for this
/// patch reach the fit ([`metal_stage::dataset::pairs`]); an unreadable example is an error,
/// not a silent skip.
pub fn train_from_examples(venue: &Venue, root: &Path) -> Result<StageModel, String> {
    let patch = patch_of(venue, root);
    let dir = root.join("dataset").join("stage").join("examples");
    let mut examples = Vec::new();
    let entries = std::fs::read_dir(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    for entry in entries {
        let path = entry.map_err(|e| e.to_string())?.path();
        if path.extension().is_some_and(|x| x == "json") {
            let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            let example: metal_stage::dataset::Example =
                serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
            examples.push(example);
        }
    }
    let model = StageModel::train(&patch, &metal_stage::dataset::pairs(&examples, &patch))?;
    save_model(root, &model)?;
    Ok(model)
}

/// The stage floats of one frame expanded to one [`Signal`] per fixture. `row` is one value
/// per matrix column, in the patch's order; `None` when it is any other width.
///
/// A pan or tilt float is an offset from the aim the placement already holds: the venue pose
/// for a fine-tuned fixture, the hitbox default otherwise. So the venue pose wins, and a zero
/// float holds the aim.
pub fn expand_row(venue: &Venue, root: &Path, row: &[f32]) -> Option<BTreeMap<String, Signal>> {
    let patch = patch_of(venue, root);
    let matrix = patch.matrix();
    if row.len() != matrix.columns.len() {
        return None;
    }
    let mut signals = matrix.expand(row);
    for p in patch.placements() {
        if let Some(s) = signals.get_mut(&p.fixture) {
            s.pan += p.yaw;
            s.tilt += p.pitch;
        }
    }
    Some(signals)
}

/// The `ai.stage` row for one music row: the venue's trained weights applied to the digest's
/// music columns. `None` when the patch has no weights that fit it.
pub fn ai_row(venue: &Venue, root: &Path, music: &[f32]) -> Option<Vec<f32>> {
    let patch = patch_of(venue, root);
    load_model(root, &patch)?.infer(&patch, music).ok()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    fn stage() -> Venue {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data");
        let text = std::fs::read_to_string(root.join("agent/venues/stage.toml")).unwrap();
        Venue::parse(&text).unwrap()
    }

    /// Two placed fixtures, one wash and one moving head, hung where the grid can see them.
    fn demo() -> Patch {
        let pf = |id: &str, kind: Kind, at: [f32; 3]| PatchFixture {
            id: id.into(),
            kind,
            at,
            group: None,
            channels: vec![],
            point: None,
            pose: None,
        };
        Patch::new(
            "demo",
            vec![pf("key-1", Kind::Wash, [-3.0, 6.7, -0.8]), pf("move-1", Kind::Moving, [-2.0, 5.0, -5.0])],
        )
    }

    #[test]
    fn a_pinned_stage_point_wins_over_the_nearest() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data");
        let text = std::fs::read_to_string(root.join("agent/venues/stage.toml")).unwrap();
        let mut venue = Venue::parse(&text).unwrap();
        let first = venue.fixtures[0].id.clone();
        let nearest = patch_of(&venue, &root).placements().into_iter().find(|p| p.fixture == first).unwrap().point;
        let pinned = Point::parse("l3.mid.c").unwrap();
        assert_ne!(nearest, pinned, "the test needs a fixture that is not already there");
        venue.fixtures[0].stage_point = Some(pinned.word());
        let placed = patch_of(&venue, &root).placements().into_iter().find(|p| p.fixture == first).unwrap();
        assert_eq!(placed.point, pinned);
        // a word that is not a grid point is not a pin: back to the nearest point
        venue.fixtures[0].stage_point = Some("l1.mid.l".into());
        let placed = patch_of(&venue, &root).placements().into_iter().find(|p| p.fixture == first).unwrap();
        assert_eq!(placed.point, nearest);
    }

    #[test]
    fn the_real_stage_patch_maps() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data");
        let venue = stage();
        let patch = patch_of(&venue, &root);
        assert!(patch.fixtures.len() >= 5, "{} fixtures", patch.fixtures.len());
        let m = patch.matrix();
        assert!(m.orphans.is_empty());
        assert!(!m.columns.is_empty());
        // every float name round-trips and every placement is a valid point
        for c in &m.columns {
            assert_eq!(metal_stage::FloatName::parse(&c.to_string()), Some(*c));
        }
    }

    #[test]
    fn a_stage_point_on_a_fixture_pins_it() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data");
        let mut venue = stage();
        let i = venue.fixtures.iter().position(|f| f.at.is_some() && kind_of(f.kind).is_some()).unwrap();
        let id = venue.fixtures[i].id.clone();
        venue.fixtures[i].stage_point = Some("l2.back.l".into());
        let patch = patch_of(&venue, &root);
        let pinned = patch.fixtures.iter().find(|f| f.id == id).unwrap();
        assert_eq!(pinned.point, Some(Point::parse("l2.back.l").unwrap()));
        let placed = patch.placements().into_iter().find(|p| p.fixture == id).unwrap();
        assert_eq!(placed.point.word(), "l2.back.l");
        // Serde: absent when unset, present when set, so an old venue file still loads.
        let unset = serde_json::to_value(&venue.fixtures[(i + 1) % venue.fixtures.len()]).unwrap();
        assert!(unset.get("stage_point").is_none(), "{unset}");
        let back: crate::venue::Fixture = serde_json::from_value(serde_json::to_value(&venue.fixtures[i]).unwrap()).unwrap();
        assert_eq!(back.stage_point.as_deref(), Some("l2.back.l"));
    }

    #[test]
    fn pin_words_are_checked() {
        assert_eq!(pin_word("l2.back.l").unwrap(), "l2.back.l");
        assert!(pin_word("l1.mid.l").is_err());
        assert!(pin_word("nowhere").unwrap_err().contains("l2.back.l"));
    }

    #[test]
    fn grid_report_counts_from_the_grid_and_names_what_is_unfilled() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data");
        let mut venue = stage();
        venue.fixtures[0].at = None;
        let report = grid_report(&venue, &root);
        assert_eq!(report["points"].as_array().unwrap().len(), Point::all().len());
        assert_eq!(report["floats"].as_array().unwrap().len(), patch_of(&venue, &root).matrix().columns.len());
        let unfilled = report["unfilled"].as_array().unwrap();
        assert!(unfilled.iter().any(|u| u["fixture"] == json!(venue.fixtures[0].id)), "{unfilled:?}");
    }

    #[test]
    fn plan_png_draws_an_empty_rig_and_a_full_one() {
        let png_of = |bytes: &[u8]| {
            let dec = png::Decoder::new(std::io::Cursor::new(bytes));
            let reader = dec.read_info().unwrap();
            (reader.info().width, reader.info().height)
        };
        let empty = Patch::new("empty", Vec::new());
        let bytes = plan_png(&empty, 480, 360).unwrap();
        assert_eq!(&bytes[..8], &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]);
        assert_eq!(png_of(&bytes), (480, 360));
        let full = plan_png(&demo(), 320, 240).unwrap();
        assert_eq!(png_of(&full), (320, 240));
        assert_ne!(full, plan_png(&empty, 320, 240).unwrap(), "the fixtures are drawn");
    }

    fn real_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data")
    }

    fn stage_venue() -> Venue {
        Venue::parse(&std::fs::read_to_string(real_root().join("agent/venues/stage.toml")).unwrap()).unwrap()
    }

    /// The matrix column of `channel` that `id` is driven by.
    pub(crate) fn column_of(patch: &Patch, id: &str, channel: Channel) -> usize {
        let m = patch.matrix();
        m.columns
            .iter()
            .enumerate()
            .position(|(i, c)| c.channel == channel && m.drives[i].iter().any(|(f, _)| f == id))
            .unwrap_or_else(|| panic!("{id} has no {channel:?} float"))
    }

    /// A scratch data root for tests that write weights: `fixtures/` and `agent/` are
    /// symlinks into the real `data/`, everything else is new. Tests never write the real one.
    pub(crate) fn scratch_root(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("fw-stage-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        #[cfg(unix)]
        for dir in ["fixtures", "agent"] {
            std::os::unix::fs::symlink(real_root().join(dir), root.join(dir)).unwrap();
        }
        root
    }

    /// Music rows with a slow loudness sweep and a moving chroma (the same shape the
    /// model's own tests train on).
    fn music(i: usize) -> Vec<f32> {
        let t = i as f32;
        let mut r = vec![0.0f32; metal_schema::MUSIC];
        r[0] = 0.5 + 0.4 * (t * 0.37).sin();
        r[2] = 0.5 + 0.5 * (t * 0.11).cos();
        r[4 + (i % 12)] = 1.0;
        r
    }

    /// Writes one approved example cut for `venue`'s patch and trains the weights into `root`.
    pub(crate) fn trained(venue: &Venue, root: &Path) -> StageModel {
        let patch = patch_of(venue, root);
        let rows: Vec<Vec<f32>> = (0..120)
            .map(|i| {
                let mut r = music(i);
                r.resize(metal_schema::MUSIC + 13, 0.0);
                r
            })
            .collect();
        let mut example = metal_stage::dataset::scaffold(&patch, "song", "song-p1", "drop", 32.0, &rows, [0, 119], 1).unwrap();
        example.approved = true;
        let dir = root.join("dataset/stage/examples");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("song-p1.json"), serde_json::to_string(&example).unwrap()).unwrap();
        train_from_examples(venue, root).unwrap()
    }

    #[test]
    fn a_zero_row_holds_each_fixtures_aim() {
        let root = real_root();
        let mut venue = stage_venue();
        // A fine-tuned head: its venue pose is the aim, not the hitbox default.
        let move1 = venue.fixtures.iter_mut().find(|f| f.id == "move-1").unwrap();
        (move1.yaw, move1.pitch) = (30.0, -20.0);
        // An untuned one: no pose, so the hitbox default is the aim.
        let key1 = venue.fixtures.iter_mut().find(|f| f.id == "key-1").unwrap();
        (key1.yaw, key1.pitch) = (0.0, 0.0);

        let patch = patch_of(&venue, &root);
        let zero = vec![0.0; patch.matrix().columns.len()];
        let signals = expand_row(&venue, &root, &zero).unwrap();
        assert_eq!((signals["move-1"].pan, signals["move-1"].tilt), (30.0, -20.0), "the venue pose wins");
        let placement = patch.placements().into_iter().find(|p| p.fixture == "key-1").unwrap();
        assert!(!placement.tuned);
        assert_eq!(signals["key-1"].pan, placement.yaw, "an untuned head aims at its hitbox default");
        assert_eq!(signals["key-1"].tilt, placement.pitch);
    }

    #[test]
    fn a_pan_float_is_an_offset_from_the_venue_pose() {
        let root = real_root();
        let mut venue = stage_venue();
        let move1 = venue.fixtures.iter_mut().find(|f| f.id == "move-1").unwrap();
        (move1.yaw, move1.pitch) = (30.0, -20.0);
        let patch = patch_of(&venue, &root);
        let mut row = vec![0.0; patch.matrix().columns.len()];
        row[column_of(&patch, "move-1", Channel::Pan)] = 10.0;
        let signals = expand_row(&venue, &root, &row).unwrap();
        assert_eq!(signals["move-1"].pan, 40.0);
        assert_eq!(signals["move-1"].tilt, -20.0, "a tilt float that is zero holds the pose");
    }

    #[test]
    fn a_row_of_the_wrong_width_is_refused() {
        let root = real_root();
        let venue = stage_venue();
        assert!(expand_row(&venue, &root, &[0.0]).is_none());
    }

    #[test]
    fn ai_stage_needs_weights_that_fit_the_patch() {
        let root = scratch_root("fit");
        let venue = stage_venue();
        let patch = patch_of(&venue, &root);
        assert!(ai_row(&venue, &root, &music(0)).is_none(), "no weights yet");

        let model = trained(&venue, &root);
        assert!(load_model(&root, &patch).is_some());
        let row = ai_row(&venue, &root, &music(7)).expect("trained weights answer");
        assert_eq!(row.len(), patch.matrix().columns.len());
        assert_eq!(row, model.infer(&patch, &music(7)).unwrap());

        // A re-patch (one head gone) changes the hash: the weights are refused, not reused.
        let mut repatched = venue.clone();
        repatched.fixtures.retain(|f| f.id != "move-1");
        assert!(ai_row(&repatched, &root, &music(7)).is_none());
        assert!(train_from_examples(&repatched, &root).is_err(), "the old example is not for this patch");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_patch_without_examples_has_nothing_to_train() {
        let root = scratch_root("empty");
        assert!(train_from_examples(&stage_venue(), &root).is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The arena example fills the common matrix: lines, walls, pixel bars, variants.
    #[test]
    fn the_arena_lands_on_lines_walls_and_pixel_bars() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data");
        let text = std::fs::read_to_string(root.join("stage/arena.toml")).unwrap();
        let patch = patch_of(&Venue::parse(&text).unwrap(), &root);
        let at: std::collections::BTreeMap<String, String> = patch
            .fixtures
            .iter()
            .zip(patch.placements())
            .map(|(f, p)| (f.id.clone(), format!("{}.{}", p.point.word(), p.variant.word())))
            .collect();
        assert_eq!(at["pro-1"], "l2.pro.x.alpha");
        assert_eq!(at["pro-8"], "l2.pro.x.delta");
        assert!(at["run3-2"].starts_with("l3.run.c."), "{}", at["run3-2"]);
        assert_eq!(at["run1-3"], "l3.run.l.alpha", "back to front: the deepest wash leads");
        assert!(at["house-bar-1"].starts_with("l3.house.x."), "{}", at["house-bar-1"]);
        assert_eq!(at["wall-l-1"], "l1.run.wl.alpha", "the deepest bar of the left wall");
        assert!(at["wall-r-4"].starts_with("l1.run.wr."), "{}", at["wall-r-4"]);
        assert!(at["back-bar-1"].starts_with("l2.back.x."), "{}", at["back-bar-1"]);
        assert!(at["edge-1"].starts_with("l1.pro.x."), "{}", at["edge-1"]);
        let m = patch.matrix();
        assert!(m.orphans.is_empty(), "{:?}", m.orphans);
        assert!(m.columns.iter().any(|c| c.pixel.is_some() && c.point.word() == "l3.house.x"));
        assert!(metal_stage::common::outside(m.names()).is_empty());
    }
}
