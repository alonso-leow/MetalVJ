//! SKELETON mode: the rig.
//!
//! Quaternion rotations per node on the 8 Hz grid, sparse keyframes, undo,
//! drag-to-retime, and the agent's desk. This is the dance authoring workflow
//! and it is carried over **whole** — every route below is the one it always
//! was, moved under a `/rig/` prefix so it stops colliding with the models
//! mode's `/state`, `/open` and `/library`, and otherwise unchanged.
//!
//! Two things did leave this module, and neither is a behaviour change:
//!
//! - The **audio library** (`/audio`, `/add`, `/digest`, `/open`) moved to
//!   [`crate::author::audio`], which is now shared. It was already the same code as
//!   the float studio's, adapted; there is one of it now, and a song added
//!   here is a song the models mode can see.
//! - The **movement library** moved to [`crate::author::library`] and grew a second
//!   kind of clip. A rig movement is byte-identical on disk to what this
//!   module used to write, deliberately.

use std::path::Path;

use metal_rig::figure::Figure;
use metal_rig::frame::Track;
use metal_rig::node::Nodes;
use metal_rig::pose::Pose;
use metal_rig::region::BodyRegion;
use metal_rig::skeleton::{Side, Skeleton};
use serde_json::{json, Value};

use crate::author::audio::Ingest;
use crate::author::{Ctx, LIBRARY};

/// What the page needs about the rig before its first fetch: the skeleton, the
/// track that is open, and the song under it.
///
/// Built per request by the caller rather than once at startup, because the
/// example library can change what is open and a page that still described the
/// song named on the command line would be a library that could only list.
pub fn page_data(ctx: &Ctx) -> Value {
    let (audio, track) = {
        let open = ctx.open.lock().expect("open lock");
        (open.audio.clone(), open.track.clone())
    };
    build(ctx.skeleton, audio.as_deref(), &track)
}

// the cleanup: the raw-socket `serve`/`handle` dispatcher that used to live here
// moved to `crate::call::author::rig` as typed handler functions over
// `examples`/`save` below (now `pub(crate)`) and `agent::*`, unchanged.

/// What the example library can offer, read off the disk every time it is
/// asked: a song digested while the app is running should show up in it.
///
/// **The folder is the manifest** — `songs` walks `data/audio` and matches each
/// piece of audio to its matrix and its track, so there is no index file to
/// drift out of sync with what is actually there. The queue is folded in on top
/// because a digest in flight has written nothing yet and the folder cannot see
/// it (`ingest`).
pub(crate) fn examples(audio: Option<&Path>, track: &Path, ingest: &Ingest) -> Value {
    let listing = |dir: &str, keep: fn(&str) -> bool| -> Vec<String> {
        let mut out: Vec<String> = metal_vfs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| metal_vfs::is_file(&p))
            .filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(keep))
            .map(|p| p.display().to_string())
            .collect();
        out.sort();
        out
    };
    json!({
        "songs": crate::author::audio::songs(Path::new(crate::author::SONGS), Path::new(crate::author::TRACKS), &ingest.states()),
        "sequences": listing(crate::author::TRACKS, |n| n.ends_with(".json")),
        "audio": audio.map(|p| p.display().to_string()),
        "track": track.display().to_string(),
        "formats": crate::author::audio::SOUND,
    })
}

/// Parses what the page posted into a real [`Track`] and writes it.
///
/// JSON, not TOML: a track is keyed by joint id and TOML has no integer-keyed
/// tables (`metal_rig::frame::Track`, and the test beside it). Going through
/// the type first is the point — a file this writes is a file the workspace
/// can read, and a page that invents a shape gets a 400 instead of a corrupt
/// track on disk.
pub(crate) fn save(body: &[u8], to: &Path) -> Result<usize, String> {
    let track: Track = serde_json::from_slice(body).map_err(|e| e.to_string())?;
    // The type is not the whole contract: serde takes `"0"` as a map key
    // happily, and 0 is not a node.
    track.validate().map_err(|e| e.to_string())?;
    let json = serde_json::to_string_pretty(&track).map_err(|e| e.to_string())?;
    if let Some(dir) = to.parent() {
        metal_vfs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    crate::atomic::write(to, json).map_err(|e| e.to_string())?;
    Ok(track.frames.len())
}

// ---------------------------------------------------------------------------
// the payload
// ---------------------------------------------------------------------------

/// Everything the page needs to draw and edit the rig.
///
/// Note what is sent that `fw-view` does not send: `rest`, the joint's
/// offset from its parent. The viewer receives *baked* points because it only
/// shows four fixed postures; an editor has to re-pose the body every time a
/// rotation changes, so it needs the skeleton itself and runs forward
/// kinematics in the page.
fn build(skeleton: &Skeleton, audio: Option<&Path>, track_path: &Path) -> Value {
    let regions = region_map(skeleton);
    let nodes = Nodes::of(skeleton);

    let joints: Vec<Value> = skeleton
        .joints
        .iter()
        .enumerate()
        .map(|(i, j)| {
            json!({
                "name": j.name,
                "parent": j.parent,
                // What the notation calls this joint, as against where the
                // array keeps it: negative on the axis, even right, odd left.
                // The page keys every slot it writes by this, because this is
                // what a saved track holds.
                "node": nodes.id(i as u16),
                "side": match j.side {
                    Side::Center => "center",
                    Side::Left => "left",
                    Side::Right => "right",
                },
                "region": regions[i],
                "rest": [j.rest_offset.x, j.rest_offset.y, j.rest_offset.z],
                "digit": is_digit(&j.name),
            })
        })
        .collect();

    let figures: Value = Figure::ALL
        .iter()
        .map(|&f| {
            let pose = f.pose(skeleton);
            let local: Vec<Value> = pose
                .local
                .iter()
                .map(|q| json!([q.x, q.y, q.z, q.w]))
                .collect();
            (
                f.name().to_owned(),
                json!({ "root": [pose.root.x, pose.root.y, pose.root.z], "local": local }),
            )
        })
        .collect::<serde_json::Map<_, _>>()
        .into();

    // An existing track is loaded rather than overwritten. Saving is
    // deliberate and explicit; opening the app is not an edit.
    let track = metal_vfs::read_to_string(track_path)
        .ok()
        .and_then(|t| serde_json::from_str::<Track>(&t).ok())
        .and_then(|t| serde_json::to_value(t).ok());

    let library: Value = metal_vfs::read_to_string(LIBRARY)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_else(|| json!([]));

    json!({
        "rig": skeleton.name,
        "joints": joints,
        "figures": figures,
        // Every joint's rest place, relative to the root. No longer a zero
        // anything is measured from — a rotation's zero is the identity — but
        // still what the page draws the unposed rig with, and still sent rather
        // than recomputed: two implementations of the rig's geometry,
        // disagreeing by a millimetre, is a whole corpus authored a millimetre
        // off.
        "restFrame": Pose::rest_frame(skeleton)
            .iter()
            .map(|p| json!([p.x, p.y, p.z]))
            .collect::<Vec<_>>(),
        "rate": metal_rig::motion::KEY_RATE,
        "track": track,
        "trackPath": track_path.display().to_string(),
        "library": library,
        "hasAudio": audio.is_some(),
        "audioName": audio.map(|p| p.display().to_string()),
    })
}

fn region_map(skeleton: &Skeleton) -> Vec<String> {
    let mut out = vec!["torso".to_owned(); skeleton.len()];
    for region in [
        BodyRegion::Head,
        BodyRegion::Torso,
        BodyRegion::LeftArm,
        BodyRegion::RightArm,
        BodyRegion::LeftHand,
        BodyRegion::RightHand,
        BodyRegion::LeftLeg,
        BodyRegion::RightLeg,
    ] {
        for j in region.joints(skeleton) {
            out[j as usize] = region.name().to_owned();
        }
    }
    out
}

fn is_digit(name: &str) -> bool {
    metal_rig::rig::DIGITS
        .iter()
        .any(|d| name.ends_with(&format!("_{d}")) || name.contains(&format!("_{d}_")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use metal_rig::frame::Frame;
    use metal_rig::node::NodeId as _Id;

    fn nowhere() -> std::path::PathBuf {
        std::path::PathBuf::from("/dev/null/nope.json")
    }

    fn payload(s: &Skeleton) -> Value {
        build(s, None, &nowhere())
    }

    /// The page cannot draw a body it cannot re-pose, so `rest` has to be in
    /// the payload for every joint — this is the one field `fw-view` gets
    /// away with omitting and an editor does not.
    #[test]
    fn the_payload_carries_the_skeleton_not_just_baked_points() {
        let s = metal_rig::rig::canonical();
        let data = payload(&s);
        let joints = data["joints"].as_array().unwrap();
        assert_eq!(joints.len(), s.len());
        assert_eq!(joints.len(), 36);
        for j in joints {
            assert!(
                j["rest"].as_array().is_some(),
                "{} has no rest offset",
                j["name"]
            );
        }
        assert_eq!(data["rate"], 8.0);
        assert!(
            data["figures"]["standing"]["local"]
                .as_array()
                .unwrap()
                .len()
                == s.len()
        );
    }

    #[test]
    fn the_jaw_and_the_thumb_reach_the_page() {
        let s = metal_rig::rig::canonical();
        let data = payload(&s);
        let names: Vec<&str> = data["joints"]
            .as_array()
            .unwrap()
            .iter()
            .map(|j| j["name"].as_str().unwrap())
            .collect();
        assert!(names.contains(&"jaw"));
        assert!(names.contains(&"l_thumb_tip"));
    }

    /// What the browser posts must survive becoming a real `Track`. This is
    /// the guarantee that the app cannot write a file the workspace cannot
    /// read — the shape is checked by the type, not by the page.
    #[test]
    fn a_posted_track_round_trips_through_the_real_type() {
        let dir = std::env::temp_dir().join("fw-studio-test");
        let path = dir.join("t.track.json");
        let _ = metal_vfs::remove_file(&path);

        // Node ids, so: an axial node, a left node and a right node — the
        // three kinds, and the negative one is the one a JSON key could
        // plausibly mangle.
        let body = serde_json::json!({
            "name": "first",
            "rig": "koreo.canonical",
            "joints": 36,
            "rate": 8.0,
            "frame": [
                { "turns": { "-3": [0.1, 0.0, -0.2], "5": [0.0, 0.4, 0.0], "6": [0.0, -0.4, 0.0] } },
                { "turns": {} }
            ]
        });
        let ticks = save(body.to_string().as_bytes(), &path).expect("save");
        assert_eq!(ticks, 2);

        let back: Track = serde_json::from_str(&metal_vfs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(back.frames.len(), 2);
        assert!(
            back.frames[1].is_empty(),
            "the empty tick survived as empty"
        );
        assert_eq!(
            back.frames[0].get(-3 as _Id),
            Some(glam::Vec3::new(0.1, 0.0, -0.2))
        );
        assert_eq!(back.frames[0].counts(36), (3, 33));
    }

    /// The page draws with array indices and saves with node ids, so the
    /// payload has to carry both and they have to agree. A wrong id here is a
    /// track that poses a different joint than the one that was dragged.
    #[test]
    fn the_payload_carries_the_notations_node_id_for_every_joint() {
        let s = metal_rig::rig::canonical();
        let data = payload(&s);
        let joints = data["joints"].as_array().unwrap();
        let node = |name: &str| {
            joints.iter().find(|j| j["name"] == name).unwrap()["node"]
                .as_i64()
                .unwrap()
        };
        assert_eq!(node("pelvis"), -1);
        assert!(node("vert_t10") < 0, "the column is negative");
        assert!(
            node("l_elbow") > 0 && node("l_elbow") % 2 == 1,
            "left is odd"
        );
        assert_eq!(
            node("r_elbow"),
            node("l_elbow") + 1,
            "right is the even partner"
        );
        assert!(
            joints.iter().all(|j| j["node"].as_i64() != Some(0)),
            "0 is not a node"
        );
    }

    #[test]
    fn nonsense_is_refused_rather_than_written() {
        let path = std::env::temp_dir().join("fw-studio-test/bad.json");
        assert!(save(b"{\"frame\": \"not a list\"}", &path).is_err());
        assert!(save(b"not json at all", &path).is_err());
        // Shaped like a track, and still not one: 0 is not a node id, and
        // this is the path that used to accept it.
        let zero = br#"{"name":"z","rig":"koreo.canonical","joints":36,"rate":8.0,
                       "frame":[{"turns":{"0":[0.0,0.1,0.0]}}]}"#;
        assert!(save(zero, &path).is_err());
        assert!(!metal_vfs::exists(&path), "a refused track must not reach the disk");
    }

    #[test]
    fn every_joint_lands_in_a_named_region_for_the_node_tree() {
        let s = metal_rig::rig::canonical();
        for (i, region) in region_map(&s).iter().enumerate() {
            assert!(!region.is_empty(), "{} has no region", s.joints[i].name);
        }
        let map = region_map(&s);
        assert_eq!(map[s.find("jaw").unwrap() as usize], "head");
        assert_eq!(map[s.find("l_thumb_tip").unwrap() as usize], "left_hand");
    }

    /// One shape on the wire, where there used to be three. A node is a
    /// rotation, so a tick serializes as a flat map of three-number arrays and
    /// the page has nothing to tag.
    #[test]
    fn a_frame_is_what_the_page_will_post_back() {
        let frame = Frame::default()
            .turn(-3, glam::Vec3::new(0.1, 0.0, -0.2))
            .turn(5, glam::Vec3::ZERO);
        let v = serde_json::to_value(&frame).unwrap();
        let turn = v["turns"]["-3"].as_array().unwrap();
        // f32 through JSON, so compare as numbers rather than as literals.
        assert!((turn[0].as_f64().unwrap() - 0.1).abs() < 1e-6);
        assert!((turn[2].as_f64().unwrap() + 0.2).abs() < 1e-6);
        // A zero rotation is a value, not a tag and not an omission.
        assert_eq!(v["turns"]["5"], json!([0.0, 0.0, 0.0]));
        assert!(v.get("slots").is_none(), "the role-tagged shape is gone");
    }
}
