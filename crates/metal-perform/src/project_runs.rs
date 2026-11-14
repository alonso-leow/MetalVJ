//! Project runs: one song's project (`data/projects/<song>.json`) played on
//! the air as a set of [`overlay::Play`]s, one per model clip.
//!
//! The rows are not a new mechanism. Each clip's keyframes become a dense
//! tick list (held between keys, the way a movement's ticks are already
//! eighths of a second), and the play is armed on [`Overlays`] exactly as
//! `POST /overlay {op:"play"}` arms one. Every frame then samples it through
//! [`Overlays::row`] / [`Play::at`], so the sampling is the overlay's own and
//! the width guard (`showing == model`) applies to each clip unchanged.
//!
//! What this module owns is the run state: which songs are running, when they
//! started, and which device/layer pairs they armed, so a run can be stopped
//! or reported as one thing rather than as loose plays.

use serde_json::{json, Value};

use crate::digest;
use crate::overlay::{Overlays, Play};

/// One armed song.
#[derive(Clone, Debug, PartialEq)]
pub struct ProjectRun {
    pub song: String,
    /// `State::time()` when it was armed.
    pub started: f32,
    /// Whether every play repeats. A looping run has no end.
    pub looping: bool,
    /// The (device, layer) each clip armed, the stop targets.
    pub targets: Vec<(String, String)>,
    /// When the longest one-shot clip is over; `None` while looping.
    pub end: Option<f32>,
}

impl ProjectRun {
    /// Whether the run is still in the air at `time`.
    pub fn running_at(&self, time: f32) -> bool {
        match self.end {
            None => true,
            Some(end) => time < end,
        }
    }
}

/// Every song running now. At most one run per song.
#[derive(Default)]
pub struct ProjectRuns {
    runs: Vec<ProjectRun>,
}

/// Builds one [`Play`] per model clip of a project file.
///
/// `resolve` maps a model id to its index and row width in this build, or
/// `None` for a model this build does not have. Clips with no `model` (video,
/// effect, fixture and group lanes) are not overlay rows and are skipped.
/// Keyframes are sorted by `tick` and held until the next one; a row whose
/// width is not the model's is refused by name, never armed.
pub fn plays_from_project(
    project: &Value,
    started: f32,
    looping: bool,
    resolve: impl Fn(&str) -> Option<(usize, usize)>,
) -> Result<Vec<Play>, String> {
    let clips = project["clips"].as_array().cloned().unwrap_or_default();
    let mut plays = Vec::new();
    for (n, clip) in clips.iter().enumerate() {
        let Some(model_id) = clip["model"].as_str().filter(|m| !m.is_empty()) else {
            continue;
        };
        let (model, width) = resolve(model_id)
            .ok_or_else(|| format!("clip {n}: {model_id:?} is not a model this build has"))?;
        let device = clip["device"].as_str().filter(|s| !s.is_empty());
        let layer = clip["layer"].as_str().filter(|s| !s.is_empty());
        let (Some(device), Some(layer)) = (device, layer) else {
            return Err(format!("clip {n}: a {model_id} clip needs a device and a layer"));
        };
        let mut keys: Vec<(usize, Vec<f32>)> = clip["keyframes"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|k| {
                let tick = k["tick"].as_u64().unwrap_or(0) as usize;
                let floats: Vec<f32> = k["floats"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|f| f.as_f64().unwrap_or(0.0) as f32)
                    .collect();
                (tick, floats)
            })
            .collect();
        keys.sort_by_key(|(t, _)| *t);
        if keys.is_empty() {
            continue;
        }
        if let Some((tick, row)) = keys.iter().find(|(_, r)| r.len() != width) {
            return Err(format!(
                "clip {n}: tick {tick} is {} floats, {model_id} takes {width}",
                row.len()
            ));
        }
        // Dense ticks: each keyframe holds until the next one.
        let last = keys.last().map_or(0, |(t, _)| *t);
        let mut ticks = Vec::with_capacity(last + 1);
        let mut k = 0;
        for i in 0..=last {
            while k + 1 < keys.len() && keys[k + 1].0 <= i {
                k += 1;
            }
            ticks.push(keys[k].1.clone());
        }
        plays.push(Play {
            device: device.to_string(),
            layer: layer.to_string(),
            movement: format!("{model_id}#{n}"),
            model,
            ticks,
            started,
            looping,
            mask: clip["mask"].as_str().map(str::to_string),
        });
    }
    Ok(plays)
}

impl ProjectRuns {
    /// Arms `plays` for `song` on `overlays`, replacing any run of the same
    /// song (its layers are released first). Returns how many plays armed.
    pub fn start(&mut self, overlays: &mut Overlays, song: &str, plays: Vec<Play>) -> usize {
        self.stop(overlays, song);
        let looping = plays.first().is_some_and(|p| p.looping);
        let started = plays.first().map_or(0.0, |p| p.started);
        let end = if looping {
            None
        } else {
            plays
                .iter()
                .map(|p| started + p.ticks.len() as f32 / digest::RATE)
                .reduce(f32::max)
                .or(Some(started))
        };
        let targets: Vec<(String, String)> =
            plays.iter().map(|p| (p.device.clone(), p.layer.clone())).collect();
        let count = plays.len();
        for play in plays {
            overlays.start(play);
        }
        self.runs.push(ProjectRun {
            song: song.to_string(),
            started,
            looping,
            targets,
            end,
        });
        count
    }

    /// Releases a song's layers. True if it was running.
    pub fn stop(&mut self, overlays: &mut Overlays, song: &str) -> bool {
        let Some(at) = self.runs.iter().position(|r| r.song == song) else {
            return false;
        };
        let run = self.runs.remove(at);
        for (device, layer) in &run.targets {
            overlays.stop(device, layer);
        }
        true
    }

    /// Releases every song. How many were running.
    pub fn stop_all(&mut self, overlays: &mut Overlays) -> usize {
        let songs: Vec<String> = self.runs.iter().map(|r| r.song.clone()).collect();
        songs.iter().filter(|s| self.stop(overlays, s)).count()
    }

    /// Whether `song` is in the air at `time`.
    pub fn running(&self, song: &str, time: f32) -> bool {
        self.runs
            .iter()
            .any(|r| r.song == song && r.running_at(time))
    }

    /// The runs, for `/state` and the grid.
    pub fn report(&self, time: f32) -> Value {
        Value::Array(
            self.runs
                .iter()
                .map(|r| {
                    json!({
                        "song": r.song,
                        "started": r.started,
                        "loop": r.looping,
                        "layers": r.targets.len(),
                        "running": r.running_at(time),
                    })
                })
                .collect(),
        )
    }

    pub fn is_empty(&self) -> bool {
        self.runs.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clip(model: &str, device: &str, layer: &str, keys: &[(u64, Vec<f32>)]) -> Value {
        json!({
            "model": model,
            "device": device,
            "layer": layer,
            "keyframes": keys.iter().map(|(t, f)| json!({"tick": t, "floats": f})).collect::<Vec<_>>(),
        })
    }

    fn resolve(id: &str) -> Option<(usize, usize)> {
        match id {
            "turing" => Some((0, 2)),
            "geo" => Some((1, 3)),
            _ => None,
        }
    }

    #[test]
    fn keyframes_are_held_into_dense_ticks() {
        let project = json!({ "song": "s", "clips": [
            clip("turing", "MAIN", "a", &[(2, vec![1.0, 1.0]), (0, vec![0.0, 0.5])]),
        ]});
        let plays = plays_from_project(&project, 3.0, false, resolve).expect("plays");
        assert_eq!(plays.len(), 1);
        let p = &plays[0];
        assert_eq!(p.ticks, vec![vec![0.0, 0.5], vec![0.0, 0.5], vec![1.0, 1.0]]);
        assert_eq!((p.device.as_str(), p.layer.as_str(), p.model), ("MAIN", "a", 0));
        assert_eq!(p.started, 3.0);
    }

    #[test]
    fn a_wrong_width_row_or_unknown_model_is_refused_by_name() {
        let bad_width = json!({ "clips": [clip("turing", "MAIN", "a", &[(0, vec![0.0])])]});
        let err = plays_from_project(&bad_width, 0.0, false, resolve).err().expect("refused");
        assert!(err.contains("turing takes 2"), "{err}");
        let unknown = json!({ "clips": [clip("nope", "MAIN", "a", &[(0, vec![0.0])])]});
        assert!(plays_from_project(&unknown, 0.0, false, resolve).err().expect("refused").contains("nope"));
    }

    #[test]
    fn non_model_clips_are_skipped() {
        let project = json!({ "clips": [
            { "video": "x.mov", "device": "MAIN", "layer": "a", "keyframes": [] },
            clip("geo", "MAIN", "b", &[(0, vec![0.0, 0.0, 0.0])]),
        ]});
        let plays = plays_from_project(&project, 0.0, true, resolve).expect("plays");
        assert_eq!(plays.len(), 1);
        assert_eq!(plays[0].layer, "b");
        assert!(plays[0].looping);
    }

    #[test]
    fn a_run_arms_its_layers_samples_rows_and_stops_cleanly() {
        let project = json!({ "clips": [
            clip("turing", "MAIN", "a", &[(0, vec![0.25, 0.75]), (1, vec![1.0, 0.0])]),
        ]});
        let plays = plays_from_project(&project, 10.0, false, resolve).expect("plays");
        let mut overlays = Overlays::default();
        let mut runs = ProjectRuns::default();
        assert_eq!(runs.start(&mut overlays, "song", plays), 1);
        assert!(runs.running("song", 10.1));
        // The overlay samples the row for the tick the clock is on.
        assert_eq!(overlays.row("MAIN", "a", 0, 10.0), Some(&[0.25, 0.75][..]));
        assert_eq!(overlays.row("MAIN", "a", 0, 10.0 + 1.5 / digest::RATE), Some(&[1.0, 0.0][..]));
        // Not the movement's own model: no row, the guard holds.
        assert_eq!(overlays.row("MAIN", "a", 1, 10.0), None);
        // Past the two ticks the one-shot is over and the run is too.
        let end = 10.0 + 2.0 / digest::RATE;
        assert!(!runs.running("song", end + 0.01));
        assert!(runs.stop(&mut overlays, "song"));
        assert!(!runs.stop(&mut overlays, "song"));
        assert!(overlays.row("MAIN", "a", 0, 10.0).is_none());
        assert!(runs.is_empty());
    }

    #[test]
    fn restarting_a_song_replaces_its_run_and_a_loop_never_ends() {
        let mut overlays = Overlays::default();
        let mut runs = ProjectRuns::default();
        let looped = plays_from_project(
            &json!({ "clips": [clip("geo", "MAIN", "b", &[(0, vec![0.0; 3])])]}),
            0.0,
            true,
            resolve,
        )
        .expect("plays");
        runs.start(&mut overlays, "s", looped);
        assert!(runs.running("s", 1.0e6));
        let again = plays_from_project(
            &json!({ "clips": [clip("geo", "MAIN", "c", &[(0, vec![0.0; 3])])]}),
            5.0,
            false,
            resolve,
        )
        .expect("plays");
        runs.start(&mut overlays, "s", again);
        // The first run's layer was released, the second one's armed.
        assert!(overlays.row("MAIN", "b", 1, 5.0).is_none());
        assert!(overlays.row("MAIN", "c", 1, 5.0).is_some());
        assert_eq!(runs.stop_all(&mut overlays), 1);
    }
}
