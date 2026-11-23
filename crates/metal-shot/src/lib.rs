//! The shot bundle: paired frames from two camera angles, on one clock.
//!
//! A shot is written on the VPS by `metal-ingest` and read at home by the
//! editor, so the format is the contract between them. It is a plain
//! directory — inspectable with `ls`, resumable with `rsync`, and diffable —
//! rather than a container that has to be unpacked before anything can look
//! at it.
//!
//! ```text
//! reverie-take-3.shot/
//!   manifest.toml
//!   tracks.csv          annotations, written at home
//!   base/a/000000.jpg   sparse pass over the whole take
//!   base/b/000000.jpg
//!   w1/a/000000.jpg     dense pass over one segment
//! ```
//!
//! The single most important property: **both cameras are extracted onto one
//! shot clock**, with the measured offset already applied. Frame `i` of `a`
//! and frame `i` of `b` are the same instant, so the editor never has to know
//! the two cameras started at different times and triangulation can trust the
//! pairing.

use metal_vfs as fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const MANIFEST: &str = "manifest.toml";
pub const TRACKS: &str = "tracks.csv";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub name: String,
    pub created_unix: u64,
    pub sync: Sync,
    /// Set when the take was also aligned to the recording the score was
    /// transcribed against — the link that puts notes, frames, and motion on
    /// one timeline.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score: Option<ScoreLink>,
    /// Bodies in this shot. Empty means the single implicit subject 0.
    #[serde(rename = "subject", default)]
    pub subjects: Vec<Subject>,
    #[serde(rename = "camera")]
    pub cameras: Vec<Camera>,
    #[serde(rename = "pass", default)]
    pub passes: Vec<Pass>,
}

/// Stable identifier for one body in a shot.
pub type SubjectId = u16;

/// A body being tracked, and what to call it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Subject {
    pub id: SubjectId,
    /// What a person calls them: "the operator", "dancer left".
    pub label: String,
}

/// Ties the shot clock to the reference recording behind the score.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScoreLink {
    pub reference: String,
    /// Seconds to add to a shot-clock time to get a time in the recording.
    pub offset_seconds: f64,
    pub confidence: f32,
}

/// How the two cameras were put on a common clock.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Sync {
    /// Seconds to add to a shot-clock time to get camera B's own time. The
    /// shot clock is camera A's clock, so this is how far B lags A.
    pub offset_seconds: f64,
    /// `audio_xcorr` or `manual`.
    pub method: String,
    /// Peak correlation, `0..=1`. Below ~0.5 the alignment is a guess.
    pub confidence: f32,
    /// Best rival peak as a fraction of the winner. Near 1.0 means the take
    /// is periodic enough that the offset could be off by a whole bar.
    pub ambiguity: f32,
    /// Worst-case pairing error, from the source frame intervals. Two cameras
    /// sampled at 30 fps cannot be paired closer than half a frame each.
    pub pair_precision_ms: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Camera {
    /// `a` or `b`.
    pub id: String,
    pub source: String,
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    pub duration: f64,
}

/// One extraction over a time range. The whole take is just the pass named
/// `base`; dense re-extractions of a segment are passes of their own, so a
/// six-second phrase can be pulled at 60 fps without re-sampling the take.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pass {
    pub id: String,
    /// Shot-clock time of frame 0.
    pub start: f64,
    /// Frames per second extracted.
    pub rate: f64,
    pub count: u32,
}

impl Pass {
    pub fn time(&self, index: u32) -> Option<f64> {
        (index < self.count).then(|| self.start + index as f64 / self.rate)
    }

    pub fn end(&self) -> f64 {
        self.start + self.count.saturating_sub(1) as f64 / self.rate
    }

    /// Nearest frame index to a shot-clock time.
    pub fn index_at(&self, time: f64) -> Option<u32> {
        if self.count == 0 {
            return None;
        }
        let i = ((time - self.start) * self.rate).round();
        (i >= 0.0 && i < self.count as f64).then_some(i as u32)
    }
}

#[derive(Debug)]
pub struct Shot {
    root: PathBuf,
    pub manifest: Manifest,
}

impl Shot {
    pub fn create(root: impl AsRef<Path>, manifest: Manifest) -> Result<Self, ShotError> {
        let root = root.as_ref().to_path_buf();
        fs::create_dir_all(&root)?;
        let shot = Shot { root, manifest };
        shot.save()?;
        Ok(shot)
    }

    pub fn open(root: impl AsRef<Path>) -> Result<Self, ShotError> {
        let root = root.as_ref().to_path_buf();
        let text = fs::read_to_string(root.join(MANIFEST))?;
        Ok(Shot {
            manifest: toml::from_str(&text)?,
            root,
        })
    }

    pub fn save(&self) -> Result<(), ShotError> {
        let text = toml::to_string_pretty(&self.manifest)?;
        fs::write(self.root.join(MANIFEST), text)?;
        Ok(())
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn pass(&self, id: &str) -> Option<&Pass> {
        self.manifest.passes.iter().find(|p| p.id == id)
    }

    pub fn camera(&self, id: &str) -> Option<&Camera> {
        self.manifest.cameras.iter().find(|c| c.id == id)
    }

    pub fn frame_dir(&self, pass: &str, camera: &str) -> PathBuf {
        self.root.join(pass).join(camera)
    }

    pub fn frame_path(&self, pass: &str, camera: &str, index: u32) -> PathBuf {
        // ffmpeg's image2 muxer counts from 1; the format hides that.
        self.frame_dir(pass, camera)
            .join(format!("{:06}.jpg", index + 1))
    }

    pub fn read_frame(&self, pass: &str, camera: &str, index: u32) -> Result<Vec<u8>, ShotError> {
        Ok(fs::read(self.frame_path(pass, camera, index))?)
    }

    /// The two views of one instant — what the editor draws side by side.
    pub fn read_pair(&self, pass_id: &str, index: u32) -> Result<Pair, ShotError> {
        let pass = self
            .pass(pass_id)
            .ok_or_else(|| ShotError::NoPass(pass_id.to_owned()))?;
        let time = pass
            .time(index)
            .ok_or(ShotError::OutOfRange { index, count: pass.count })?;
        Ok(Pair {
            index,
            time,
            a: self.read_frame(pass_id, "a", index)?,
            b: self.read_frame(pass_id, "b", index)?,
        })
    }

    /// When a frame was really taken, after the camera's own frame grid
    /// rounded the request.
    ///
    /// Extraction asks for a time; a camera can only answer with a frame it
    /// actually has. This is that answer, and it is what triangulation should
    /// use — not the nominal pass time.
    pub fn realized_time(&self, pass_id: &str, camera_id: &str, index: u32) -> Option<f64> {
        let pass = self.pass(pass_id)?;
        let camera = self.camera(camera_id)?;
        let wanted = pass.time(index)?
            + if camera_id == "b" {
                self.manifest.sync.offset_seconds
            } else {
                0.0
            };
        Some((wanted * camera.fps).round() / camera.fps)
    }

    /// How far apart in time the two views of one pair really are, in seconds.
    ///
    /// Never zero except by luck: two cameras running at different rates
    /// almost never expose at the same instant. Worth surfacing in the editor
    /// on any pair where it approaches a frame, because that is where a fast
    /// hand will not triangulate cleanly.
    pub fn pair_error(&self, pass_id: &str, index: u32) -> Option<f64> {
        let a = self.realized_time(pass_id, "a", index)?;
        let b = self.realized_time(pass_id, "b", index)?;
        Some((a - (b - self.manifest.sync.offset_seconds)).abs())
    }

    /// Worst and mean pairing error over a pass, in seconds.
    pub fn pair_error_stats(&self, pass_id: &str) -> Option<(f64, f64)> {
        let pass = self.pass(pass_id)?;
        let errors: Vec<f64> = (0..pass.count)
            .filter_map(|i| self.pair_error(pass_id, i))
            .collect();
        if errors.is_empty() {
            return None;
        }
        let worst = errors.iter().cloned().fold(0.0, f64::max);
        Some((worst, errors.iter().sum::<f64>() / errors.len() as f64))
    }

    /// Total bytes on disk, for the storage guard.
    pub fn size_bytes(&self) -> u64 {
        fn walk(dir: &Path) -> u64 {
            fs::read_dir(dir)
                .map(|entries| {
                    entries
                        .flatten()
                        .map(|e| match e.file_type() {
                            Ok(t) if t.is_dir() => walk(&e.path()),
                            _ => e.metadata().map(|m| m.len()).unwrap_or(0),
                        })
                        .sum()
                })
                .unwrap_or(0)
        }
        walk(&self.root)
    }

    pub fn tracks_path(&self) -> PathBuf {
        self.root.join(TRACKS)
    }

    pub fn read_tracks(&self) -> Result<Vec<Observation>, ShotError> {
        let path = self.tracks_path();
        if !metal_vfs::exists(&path) {
            return Ok(Vec::new());
        }
        fs::read_to_string(path)?
            .lines()
            .skip(1)
            .filter(|l| !l.trim().is_empty())
            .map(Observation::parse)
            .collect()
    }

    pub fn write_tracks(&self, observations: &[Observation]) -> Result<(), ShotError> {
        let mut out = fs::File::create(self.tracks_path())?;
        writeln!(out, "pass,frame,subject,camera,joint,x,y,confidence")?;
        for o in observations {
            writeln!(
                out,
                "{},{},{},{},{},{:.3},{:.3},{:.3}",
                o.pass, o.frame, o.subject, o.camera, o.joint, o.x, o.y, o.confidence
            )?;
        }
        Ok(())
    }
}

/// One instant, seen from both angles.
#[derive(Debug, Clone)]
pub struct Pair {
    pub index: u32,
    /// Shot-clock time in seconds.
    pub time: f64,
    pub a: Vec<u8>,
    pub b: Vec<u8>,
}

/// A joint located in one camera's image plane.
///
/// CSV rather than TOML: a take annotated densely runs to hundreds of
/// thousands of rows, and this stays greppable and small.
#[derive(Debug, Clone, PartialEq)]
pub struct Observation {
    pub pass: String,
    pub frame: u32,
    /// Which body this joint belongs to.
    ///
    /// Single-dancer work leaves this at 0 forever. It exists from the start
    /// anyway, because adding a subject axis to a format that already holds a
    /// season of annotation is a migration, and adding it to an empty one is a
    /// column. The identity it carries is the hard part of multi-person
    /// capture: subject 1 in camera A must be the same body as subject 1 in
    /// camera B, on every frame, or triangulation silently fuses two people.
    pub subject: SubjectId,
    /// `a` or `b`.
    pub camera: String,
    /// Joint name, matching the rig.
    pub joint: String,
    /// Pixels, origin top-left of the extracted frame.
    pub x: f32,
    pub y: f32,
    /// `1.0` for a human-placed point, lower for a machine estimate. The
    /// distinction is the whole basis of trusting the ground truth later.
    pub confidence: f32,
}

impl Observation {
    fn parse(line: &str) -> Result<Self, ShotError> {
        let f: Vec<&str> = line.split(',').collect();
        let bad = || ShotError::BadTrack(line.to_owned());
        if f.len() != 8 {
            return Err(bad());
        }
        Ok(Observation {
            pass: f[0].to_owned(),
            frame: f[1].parse().map_err(|_| bad())?,
            subject: f[2].parse().map_err(|_| bad())?,
            camera: f[3].to_owned(),
            joint: f[4].to_owned(),
            x: f[5].parse().map_err(|_| bad())?,
            y: f[6].parse().map_err(|_| bad())?,
            confidence: f[7].parse().map_err(|_| bad())?,
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ShotError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("manifest is not valid: {0}")]
    Decode(#[from] toml::de::Error),
    #[error("manifest could not be written: {0}")]
    Encode(#[from] toml::ser::Error),
    #[error("no pass named {0:?}")]
    NoPass(String),
    #[error("frame {index} is outside the pass ({count} frames)")]
    OutOfRange { index: u32, count: u32 },
    #[error("malformed track row: {0}")]
    BadTrack(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest() -> Manifest {
        Manifest {
            name: "take".into(),
            created_unix: 0,
            sync: Sync {
                offset_seconds: 1.25,
                method: "audio_xcorr".into(),
                confidence: 0.9,
                ambiguity: 0.2,
                pair_precision_ms: (0.5 / 30.0 + 0.5 / 25.0) * 1000.0,
            },
            score: None,
            subjects: Vec::new(),
            cameras: vec![
                Camera {
                    id: "a".into(),
                    source: "left.mp4".into(),
                    width: 1920,
                    height: 1080,
                    fps: 30.0,
                    duration: 60.0,
                },
                Camera {
                    id: "b".into(),
                    source: "right.mp4".into(),
                    width: 1920,
                    height: 1080,
                    fps: 25.0,
                    duration: 62.0,
                },
            ],
            passes: vec![Pass {
                id: "base".into(),
                start: 0.0,
                rate: 10.0,
                count: 600,
            }],
        }
    }

    #[test]
    fn a_pass_maps_indices_to_shot_time_and_back() {
        let p = &manifest().passes[0];
        assert_eq!(p.time(0), Some(0.0));
        assert_eq!(p.time(10), Some(1.0));
        assert_eq!(p.time(600), None);
        assert_eq!(p.index_at(1.04), Some(10));
        assert_eq!(p.index_at(-1.0), None);
        assert!((p.end() - 59.9).abs() < 1e-9);
    }

    #[test]
    fn a_manifest_round_trips() {
        let dir = std::env::temp_dir().join(format!("metal-shot-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let shot = Shot::create(&dir, manifest()).unwrap();
        let back = Shot::open(&dir).unwrap();
        assert_eq!(back.manifest.name, "take");
        assert_eq!(back.manifest.sync.offset_seconds, 1.25);
        assert_eq!(back.camera("b").unwrap().duration, 62.0);
        assert_eq!(back.pass("base").unwrap().count, 600);
        assert!(back.pass("nope").is_none());
        drop(shot);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn realized_times_land_on_each_cameras_own_frame_grid() {
        let shot = Shot {
            root: PathBuf::from("/nowhere"),
            manifest: manifest(), // a at 30 fps, b at 25 fps, offset +1.25 s
        };
        // Pass frame 3 is at 0.3 s. Camera A can only answer on 1/30 s.
        let a = shot.realized_time("base", "a", 3).unwrap();
        assert!((a * 30.0 - (a * 30.0).round()).abs() < 1e-9);
        assert!((a - 0.3).abs() <= 0.5 / 30.0);
        // Camera B is asked for 0.3 + 1.25 and answers on 1/25 s.
        let b = shot.realized_time("base", "b", 3).unwrap();
        assert!((b * 25.0 - (b * 25.0).round()).abs() < 1e-9);
        assert!((b - 1.55).abs() <= 0.5 / 25.0);
    }

    #[test]
    fn pairing_error_stays_inside_the_bound_the_manifest_declares() {
        let shot = Shot {
            root: PathBuf::from("/nowhere"),
            manifest: manifest(),
        };
        let bound = shot.manifest.sync.pair_precision_ms / 1000.0;
        let (worst, mean) = shot.pair_error_stats("base").unwrap();
        assert!(worst <= bound, "worst {worst} exceeds declared {bound}");
        assert!(mean <= worst);
        // Two cameras at different rates: some pairs really are imperfect.
        assert!(worst > 0.0, "a 30/25 fps pair cannot be exact everywhere");
    }

    #[test]
    fn frame_paths_are_one_based_on_disk_and_zero_based_in_the_api() {
        let dir = std::env::temp_dir().join("metal-shot-paths");
        let shot = Shot {
            root: dir.clone(),
            manifest: manifest(),
        };
        assert_eq!(shot.frame_path("base", "a", 0), dir.join("base/a/000001.jpg"));
        assert_eq!(shot.frame_path("base", "b", 41), dir.join("base/b/000042.jpg"));
    }

    #[test]
    fn tracks_round_trip_through_csv() {
        let dir = std::env::temp_dir().join(format!("fw-tracks-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let shot = Shot::create(&dir, manifest()).unwrap();
        assert!(shot.read_tracks().unwrap().is_empty(), "absent file is empty");

        let obs = vec![
            Observation {
                pass: "base".into(),
                frame: 12,
                subject: 0,
                camera: "a".into(),
                joint: "l_wrist".into(),
                x: 812.5,
                y: 447.25,
                confidence: 1.0,
            },
            Observation {
                pass: "base".into(),
                frame: 12,
                subject: 1,
                camera: "b".into(),
                joint: "l_finger_tip".into(),
                x: 1201.0,
                y: 390.0,
                confidence: 0.4,
            },
        ];
        shot.write_tracks(&obs).unwrap();
        assert_eq!(shot.read_tracks().unwrap(), obs);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_malformed_track_row_is_rejected_rather_than_guessed() {
        assert!(Observation::parse("base,12,0,a,l_wrist,nope,447,1.0").is_err());
        assert!(Observation::parse("base,12,a").is_err());
        // The pre-subject layout must be rejected, not silently misread.
        assert!(Observation::parse("base,12,a,l_wrist,10,20,1.0").is_err());
    }
}
