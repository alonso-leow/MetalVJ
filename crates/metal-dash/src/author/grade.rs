//! The Colourist's half of `metal_reel::grade` — turning authored `Example`s
//! into the dataset [`metal_reel::grade::train`] fits, and the route's logic.
//! Calqued off `author::router::train` (`S.AP` point 2 in `the design notes`): the
//! router turns picks into `metal_router::take::Sample`s the same shape this
//! turns keyframed floats into [`grade::Take`]s.
//!
//! # What an `Example` becomes
//!
//! An authored [`example::Example`] is sparse ticks of a model's *own*
//! floats — the row `dense::interpolate` fills in between them is the row an
//! operator said the model should have, at whatever the song was doing at
//! that tick. [`grade::Take`] wants the opposite pairing: the music, and how
//! far that row sits from the model's own rest position — so every dense
//! tick becomes one `Take`, with the song's digest at that tick and each
//! float reduced through [`grade::offset_of`] against the *renderer's own*
//! schema — not `models::schema`'s JSON mirror of it, which carries no
//! `drive` (see that module's own doc for why the two stay separate types).
//!
//! Every `.json` directly under the model's own folder counts, shared corpus
//! and every `(device, layer)` override alike (`example::override_path_for`'s
//! siblings) — training pools every song and every device's authored take
//! into one fit, because the Colourist itself is one model, not one per
//! device. Only the *authoring* is paged by device (`S.AP`'s "pages logic":
//! the lane scopes what you are looking at, not what the grade learns from).
//!
//! `examples_root`/`songs_root` are parameters rather than the crate's own
//! `EXAMPLES_DIR`/`SONGS` constants (unlike `models::export`, which reads
//! them directly) — the one thing this module needed that made it worth the
//! extra two arguments at both call sites: a real test of the success path,
//! against a scratch tree, instead of only the refusal paths `export`'s own
//! tests are limited to.

use std::collections::HashMap;
use std::path::Path;

use metal_render::model::{Block as RenderBlock, Schema};
use metal_reel::grade;
use metal_router::npy;

use crate::author::models::example;
use crate::author::models::schema::{Block as ModelBlock, ModelSchema};

/// Where a fitted grade lives — the same root [`metal_reel::grade::Colourist::
/// model_path`] joins its own index dir onto, and the same one
/// `author::curator::DIR` already uses for the same reason: this is the live
/// `data/` the booth reads, not a studio-only scratch folder.
pub const DATA: &str = "data";

/// Every example on disk for `model` under `examples_root` — shared corpus
/// and every device/layer override alike. Filenames are never parsed; each
/// file's own `song` field is what is trusted, exactly as [`example::load`]
/// trusts the file's own contents over its path.
fn example_files(examples_root: &Path, model: &str) -> Vec<example::Example> {
    let dir = examples_root.join(model);
    metal_vfs::read_dir(&dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .filter_map(|p| metal_vfs::read_to_string(p).ok())
        .filter_map(|t| serde_json::from_str::<example::Example>(&t).ok())
        .filter(|ex| !ex.ticks.is_empty())
        .collect()
}

/// One dense row's own floats, by name — walked the same way
/// `ModelSchema::defaults` walks blocks, so the offset into `row` for each
/// `Block::Floats` span always lines up with what `dense::interpolate`
/// produced from this same schema.
pub(crate) fn own_float_values(schema: &ModelSchema, row: &[f32]) -> Vec<(String, f32)> {
    let mut out = Vec::new();
    let mut at = 0usize;
    for block in &schema.blocks {
        if let ModelBlock::Floats { fields } = block {
            for (i, f) in fields.iter().enumerate() {
                if let Some(v) = row.get(at + i) {
                    out.push((f.name.clone(), *v));
                }
            }
        }
        at += block.width();
    }
    out
}

/// Every training example this model's authored `Example`s make: one
/// [`grade::Take`] per dense tick of every song that has a digested matrix,
/// each float reduced through [`grade::offset_of`] against `real_schema`'s
/// own `min`/`max`/`drive`.
///
/// Offsets are built in [`grade::float_names`]'s order, looked up by name
/// per tick, rather than trusted to line up positionally with
/// `model_schema`'s own block order — a schema that drifted between the two
/// representations degrades to a neutral (zero) offset for the float that
/// drifted, never a misaligned one.
pub fn takes_for(
    examples_root: &Path,
    songs_root: &Path,
    model: &str,
    model_schema: &ModelSchema,
    real_schema: &Schema,
) -> Vec<grade::Take> {
    let names = grade::float_names(real_schema);
    let mut specs: HashMap<&str, &metal_render::model::FloatSpec> = HashMap::new();
    for block in real_schema.blocks {
        if let RenderBlock::Floats(fields) = block {
            for f in fields.iter() {
                specs.insert(f.name, f);
            }
        }
    }

    let mut out = Vec::new();
    for ex in example_files(examples_root, model) {
        let Ok(matrix) = npy::load_song(songs_root, &ex.song, metal_schema::TickRate::CANONICAL)
        else {
            continue;
        };
        if matrix.is_empty() {
            continue;
        }
        let dense_rows =
            crate::author::models::dense::interpolate(&ex, model_schema.width(), matrix.len());
        for (tick, row) in dense_rows.iter().enumerate() {
            let values: HashMap<String, f32> =
                own_float_values(model_schema, row).into_iter().collect();
            let offsets: Vec<f32> = names
                .iter()
                .map(|n| {
                    let v = values.get(n).copied().unwrap_or(0.0);
                    match specs.get(n.as_str()) {
                        Some(spec) => {
                            grade::offset_of(v, spec.default, spec.min, spec.max, spec.drive)
                        }
                        None => 0.0,
                    }
                })
                .collect();
            out.push(grade::Take {
                song: ex.song.clone(),
                music: matrix[tick][..metal_reel::MUSIC]
                    .try_into()
                    .expect("a digest row is at least the music prefix"),
                offsets,
            });
        }
    }
    out
}

// ---------------------------------------------------------------------------
// the ingest matrix — what the columns lane paints
// ---------------------------------------------------------------------------

/// One song's digest, split into the named columns `the build reference` § "Audio
/// features: the input matrix" fixes: `0`=RMS, `1`=spectral bandwidth,
/// `2`=onset strength, `3`=spectral centroid, `4..16`=the twelve chroma
/// pitch classes. This is the *raw* per-tick digest — the same file
/// [`takes_for`] reduces into training takes — served whole so the client can
/// paint it as a scrolling strip the way `page/suite.js`'s `lanes()` once
/// drew `laneA`/`laneB` straight off `matrix`.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Matrix {
    pub song: String,
    pub rate: f32,
    pub ticks: usize,
    pub rms: Vec<f32>,
    pub bandwidth: Vec<f32>,
    pub onset: Vec<f32>,
    pub centroid: Vec<f32>,
    pub chroma: Vec<[f32; 12]>,
    /// The thirteen voice columns ([`metal_schema::VOICE_COLUMNS`] order), one
    /// row per tick. All zero when the digest was written before V2 and carries
    /// no voice block — [`Matrix::has_voice`] is what tells those apart.
    pub voice: Vec<[f32; metal_schema::VOICE]>,
    /// Whether the digest's own file carried the voice block. A 16-wide legacy
    /// file reports `false`, and the timeline says "not digested for voice"
    /// rather than drawing a silent singer (the voice design notes decision 5).
    pub has_voice: bool,
    /// Whether the only digest for this song is an 8 Hz base file with no
    /// canonical sibling — the one honest loss of the migration, held as steps
    /// at the fine level and badged "base-only" (`npy::is_base_only`).
    pub base_only: bool,
}

/// The pure split — no disk, so this is what the tests hold. `rows` is
/// exactly what [`npy::load`] returns: one digest row per tick, and only the
/// music prefix and the voice block are split out of it, in the column order
/// above. `has_voice` mirrors the file's own width (`npy::has_voice`), which a
/// zeroed voice block cannot be asked to prove.
pub fn matrix_from_rows(
    song: &str,
    rate: f32,
    rows: &[[f32; metal_router::features::COLUMNS]],
    has_voice: bool,
) -> Matrix {
    let mut m = Matrix {
        song: song.to_string(),
        rate,
        ticks: rows.len(),
        rms: Vec::with_capacity(rows.len()),
        bandwidth: Vec::with_capacity(rows.len()),
        onset: Vec::with_capacity(rows.len()),
        centroid: Vec::with_capacity(rows.len()),
        chroma: Vec::with_capacity(rows.len()),
        voice: Vec::with_capacity(rows.len()),
        has_voice,
        base_only: false,
    };
    for r in rows {
        m.rms.push(r[0]);
        m.bandwidth.push(r[1]);
        m.onset.push(r[2]);
        m.centroid.push(r[3]);
        let mut c = [0.0f32; 12];
        c.copy_from_slice(&r[4..16]);
        m.chroma.push(c);
        m.voice.push(std::array::from_fn(|j| r[metal_schema::MUSIC + j]));
    }
    m
}

/// Loads a song's digest at the canonical level and splits it — `None` for an
/// undigested or missing song, the same "nothing to draw, not an error" the
/// old lane's `matrix ? … : 'analyzing…'` branch treated it as.
///
/// The voice block and the `base_only` mark are read off the **file**, not the
/// zeroed row a 16-wide legacy matrix loads with, so the timeline can tell "the
/// singer is silent" from "this digest never measured a voice".
pub fn matrix_for(songs_root: &Path, song: &str, rate: f32) -> Option<Matrix> {
    // Canonical rows — a base-only digest is expanded as steps, so the matrix
    // and the (canonical) keyframe grid agree. `rate` is what the caller paints
    // the ruler at; it should be the canonical rate.
    let path = npy::song_path(songs_root, song, metal_schema::TickRate::CANONICAL)?;
    let has_voice = npy::has_voice(&path).unwrap_or(false);
    let base_only = npy::is_base_only(songs_root, song);
    let rows = npy::load_song(songs_root, song, metal_schema::TickRate::CANONICAL).ok()?;
    if rows.is_empty() {
        return None;
    }
    let mut m = matrix_from_rows(song, rate, &rows, has_voice);
    m.base_only = base_only;
    Some(m)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::author::models::example::Tick;
    use metal_render::model::{color, spec, spec_held};

    fn schema() -> ModelSchema {
        ModelSchema {
            id: "wake".into(),
            label: "Wake".into(),
            technique: String::new(),
            blocks: vec![
                ModelBlock::Floats {
                    fields: vec![
                        crate::author::models::schema::FloatSpec {
                            name: "stutter".into(),
                            min: 0.0,
                            max: 1.0,
                            default: 0.2,
                        },
                        crate::author::models::schema::FloatSpec {
                            name: "pinned".into(),
                            min: 0.0,
                            max: 1.0,
                            default: 0.5,
                        },
                    ],
                },
                ModelBlock::ColorSlot {
                    name: "principal".into(),
                },
            ],
        }
    }

    static REAL_FLOATS: &[metal_render::model::FloatSpec] = &[
        spec("stutter", 0.0, 1.0, 0.2),
        spec_held("pinned", 0.0, 1.0, 0.5, 0.0),
    ];
    static REAL_SCHEMA: Schema = Schema {
        blocks: &[RenderBlock::Floats(REAL_FLOATS), color("principal")],
    };

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("metal-dash-grade-{name}-{}", std::process::id()));
        let _ = metal_vfs::remove_dir_all(&dir);
        dir
    }

    /// A tiny digest matrix — onset (column 2) spiking every 4th tick, so a
    /// take's reduced `stutter` offset is not just noise around zero.
    fn write_matrix(path: &std::path::Path, rows: usize) {
        let matrix: Vec<Vec<f32>> = (0..rows)
            .map(|i| {
                let mut r = vec![0.5f32; 16];
                r[2] = if i % 4 == 0 { 0.9 } else { 0.05 };
                r
            })
            .collect();
        crate::author::models::dense::write_npy(path, &matrix, 16).expect("write matrix");
    }

    #[test]
    fn own_float_values_skips_the_colour_slot_and_keeps_names() {
        let mut row = vec![0.7f32, 0.3];
        row.extend([0.0f32; 12]); // the colour slot
        let got = own_float_values(&schema(), &row);
        assert_eq!(
            got,
            vec![
                ("stutter".to_string(), 0.7),
                ("pinned".to_string(), 0.3)
            ]
        );
    }

    #[test]
    fn every_example_file_for_a_model_is_pooled_shared_and_overrides_alike() {
        let examples_root = scratch("pool-examples");
        let songs_root = scratch("pool-songs");
        metal_vfs::create_dir_all(songs_root.join("x")).ok(); // ensure parent exists for write_matrix below
        write_matrix(&songs_root.join(metal_schema::digest_file_name("s", metal_schema::TickRate::Base)), 8);

        let base_tick = |v: f32| Tick {
            tick: 0,
            floats: vec![v, 0.5].into_iter().chain(std::iter::repeat(1.0).take(12)).collect(),
        };
        example::save(
            &examples_root,
            example::Example {
                model: "wake".into(),
                song: "s".into(),
                rate: 8.0,
                ticks: vec![base_tick(0.2)],
            },
            &schema(),
            None,
        )
        .expect("save shared");
        example::save(
            &examples_root,
            example::Example {
                model: "wake".into(),
                song: "s".into(),
                rate: 8.0,
                ticks: vec![base_tick(0.9)],
            },
            &schema(),
            Some(("dev1", "l1-1")),
        )
        .expect("save override");

        let takes = takes_for(&examples_root, &songs_root, "wake", &schema(), &REAL_SCHEMA);
        // The base-only 8-row matrix is held as 32 canonical steps; 32 takes
        // per file, 2 files (shared + override).
        assert_eq!(takes.len(), 64, "both the shared file and the override counted");
        assert!(takes.iter().all(|t| t.song == "s"));
        assert!(takes.iter().all(|t| t.offsets.len() == 2));
    }

    /// A dataset part (`author::dataset`) trains only on the ticks inside its
    /// span — the held rows before and after it are not takes.
    #[test]
    fn a_dataset_part_trains_only_inside_its_span() {
        let parts_root = scratch("part-examples");
        let songs_root = scratch("part-songs");
        metal_vfs::create_dir_all(&parts_root).unwrap();
        metal_vfs::create_dir_all(&songs_root).unwrap();
        // 10 base rows = 40 canonical ticks.
        write_matrix(&songs_root.join(metal_schema::digest_file_name("s", metal_schema::TickRate::Base)), 10);
        let row = |v: f32| -> Vec<f32> { vec![v, 0.5].into_iter().chain(std::iter::repeat(1.0).take(12)).collect() };
        let part = serde_json::json!({
            "model": "wake", "song": "s", "rate": 32.0,
            "ticks": [{ "tick": 8, "floats": row(0.2) }, { "tick": 20, "floats": row(0.8) }],
            "part": "01-p1", "span": [8, 20], "kind": "drop",
        });
        metal_vfs::write(parts_root.join("01-p1.json"), part.to_string()).unwrap();

        let takes = crate::author::dataset::takes_for(&parts_root, &songs_root, "wake", &schema(), &REAL_SCHEMA);
        assert_eq!(takes.len(), 13, "ticks 8..=20, nothing held outside the span");
        let first = takes.first().unwrap().offsets[0];
        let last = takes.last().unwrap().offsets[0];
        assert!(first.abs() < 1e-6, "tick 8 sits at the default");
        assert!(last > first, "the part rises to its last key");
        // A part of another model is not this model's take.
        assert!(crate::author::dataset::takes_for(&parts_root, &songs_root, "rings", &schema(), &REAL_SCHEMA).is_empty());
        let _ = metal_vfs::remove_dir_all(&parts_root);
        let _ = metal_vfs::remove_dir_all(&songs_root);
    }

    #[test]
    fn a_song_with_no_digested_matrix_contributes_nothing_rather_than_panicking() {
        let examples_root = scratch("no-matrix-examples");
        let songs_root = scratch("no-matrix-songs");
        example::save(
            &examples_root,
            example::Example {
                model: "wake".into(),
                song: "undigested".into(),
                rate: 8.0,
                ticks: vec![Tick {
                    tick: 0,
                    floats: std::iter::repeat(0.5).take(14).collect(),
                }],
            },
            &schema(),
            None,
        )
        .expect("save");
        let takes = takes_for(&examples_root, &songs_root, "wake", &schema(), &REAL_SCHEMA);
        assert!(takes.is_empty());
    }

    #[test]
    fn matrix_from_rows_splits_by_the_documented_column_order() {
        let mut row = [0.0f32; metal_router::features::COLUMNS];
        row[0] = 0.9; // rms
        row[1] = 0.4; // bandwidth
        row[2] = 0.7; // onset
        row[3] = 0.6; // centroid
        for (i, c) in row[4..metal_reel::MUSIC].iter_mut().enumerate() {
            *c = i as f32 / 11.0; // chroma, ramping so a transposition would show
        }
        for (j, c) in row[metal_schema::MUSIC..].iter_mut().enumerate() {
            *c = j as f32 / 12.0; // voice, ramping so an offset would show
        }
        let m = matrix_from_rows("s", 8.0, &[row], true);
        assert_eq!(m.ticks, 1);
        assert_eq!(m.rms, vec![0.9]);
        assert_eq!(m.bandwidth, vec![0.4]);
        assert_eq!(m.onset, vec![0.7]);
        assert_eq!(m.centroid, vec![0.6]);
        assert_eq!(m.chroma[0][0], 0.0);
        assert_eq!(m.chroma[0][11], 1.0);
        assert!(m.has_voice);
        assert_eq!(m.voice[0][0], 0.0);
        assert_eq!(m.voice[0][metal_schema::VOICE - 1], 1.0);
    }

    #[test]
    fn matrix_for_reads_the_songs_own_npy_and_is_none_when_undigested() {
        let songs_root = scratch("matrix-songs");
        write_matrix(&songs_root.join(metal_schema::digest_file_name("s", metal_schema::TickRate::Base)), 5);
        let m = matrix_for(&songs_root, "s", 32.0).expect("digested");
        assert_eq!(m.ticks, 20, "a base-only 5-row file is held as 20 fine steps");
        assert_eq!(m.onset[0], 0.9, "the spiking tick from write_matrix");
        assert!(!m.has_voice, "the script's 16-wide file carries no voice block");
        assert!(m.base_only, "no canonical sibling marks it base-only");
        assert!(matrix_for(&songs_root, "no-such-song", 8.0).is_none());
    }

    /// The digest's own file width and the base-only mark survive the split:
    /// a 29-wide canonical matrix reports voice present and no base-only mark,
    /// so the timeline can tell a silent singer from an unmeasured one.
    #[test]
    fn a_29_wide_canonical_digest_reports_voice_present() {
        let songs_root = scratch("matrix-voice");
        let path = songs_root.join(metal_schema::digest_file_name("v", metal_schema::TickRate::CANONICAL));
        let rows: Vec<Vec<f32>> = (0..4)
            .map(|i| {
                let mut r = vec![0.5f32; metal_schema::FEATURES];
                r[metal_schema::MUSIC] = if i % 2 == 0 { 0.8 } else { 0.1 }; // voice presence
                r
            })
            .collect();
        crate::author::models::dense::write_npy(&path, &rows, metal_schema::FEATURES).expect("write");
        let m = matrix_for(&songs_root, "v", 32.0).expect("digested");
        assert!(m.has_voice, "a 29-wide file carries the voice block");
        assert!(!m.base_only, "a canonical file is not base-only");
        assert_eq!(m.voice[0][0], 0.8);
        assert_eq!(m.voice[1][0], 0.1);
    }

    #[test]
    fn a_pinned_float_reduces_to_zero_regardless_of_the_authored_value() {
        let examples_root = scratch("pinned-examples");
        let songs_root = scratch("pinned-songs");
        write_matrix(&songs_root.join(metal_schema::digest_file_name("s", metal_schema::TickRate::Base)), 4);
        example::save(
            &examples_root,
            example::Example {
                model: "wake".into(),
                song: "s".into(),
                rate: 8.0,
                ticks: vec![Tick {
                    tick: 0,
                    floats: vec![0.2, 0.99]
                        .into_iter()
                        .chain(std::iter::repeat(1.0).take(12))
                        .collect(),
                }],
            },
            &schema(),
            None,
        )
        .expect("save");
        let takes = takes_for(&examples_root, &songs_root, "wake", &schema(), &REAL_SCHEMA);
        assert!(!takes.is_empty());
        // "pinned" is offset index 1 (`grade::float_names` order matches
        // `REAL_FLOATS`'s declaration order here).
        assert!(
            takes.iter().all(|t| t.offsets[1] == 0.0),
            "a drive-0 float must never carry a signal"
        );
    }
}
