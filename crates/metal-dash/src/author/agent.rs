//! The interface the agent uses, which is the one the page already uses.
//!
//! Today the app is driven by a hand on a keyboard. The goal
//! (`prompt-claude-koreo-autonomous-layer.md`) is that oa001 can take an
//! instruction during the day — *"gira l_shoulder a [0, 0, 1.2] en el tick
//! 42"* — apply it, capture what it looks like, keep the result in the movement
//! library, and hand the operator a day's work to supervise at the end of it.
//!
//! **The path is JSON over the server, not simulated keystrokes.** The server
//! is already where the page and the disk meet: `save` parses into a real
//! [`Track`] before anything is written, and every guarantee the app has comes
//! from that one round trip. A proposal endpoint reuses it, so the agent cannot
//! invent a shape the type does not have — which is the same protection the
//! browser gets, for free, rather than a second validator that drifts.
//!
//! Same app, two interfaces. The human keeps the keyboard.
//!
//! # What a proposal says
//!
//! A node's three numbers are a **rotation** (`crate::author::frame`, 2026-08-15): the
//! axis the joint turns about, scaled by how far, measured against its rest
//! frame. `{"id": 27, "turn": [0, 0, 1.2]}` puts that joint at 1.2 radians
//! about its parent's Z — the same statement at tick 42 as at tick 1, because a
//! local rotation is measured against a frame that travels with the body.
//!
//! The field is `turn` and it used to be `by`, which was itself a rename from
//! `pos`. Both of those were guarding against a reader who took the numbers for
//! a place; the field is now the third thing it has been in three days, and it
//! is the one the numbers actually are.
//!
//! Two ways to write a node line and no third:
//!
//! ```json
//! { "id": 27, "turn": [0, 0, 1.2] }   // this joint is turned like this
//! { "id": 27, "silent": true }        // take the statement back
//! ```
//!
//! There is no `role`. The given/carried/silent vocabulary is gone with the
//! chain solver that read it, so a proposal has nothing left to choose between.
//!
//! # Why the state still comes back replayed
//!
//! `GET /state?tick=N` answers with both what the tick *says* and where every
//! node actually *is*, and the second one costs a replay from tick 0. Not
//! because a tick is relative — it is not any more — but because the record is
//! **sparse**: the joints tick 40 declines to mention still have rotations, and
//! the only way to know them is the ticks that set them. An agent that wants to
//! check its own work needs the second number, and an agent that only got the
//! first would be reading the instruction back to itself.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use metal_rig::figure::Figure;
use metal_rig::frame::{Frame, Track};
use metal_rig::node::{NodeId, Nodes};
use metal_rig::skeleton::Skeleton;
use glam::Vec3;
use serde_json::{json, Value};

/// One proposal's worth of history: the track it touched, and every tick it
/// replaced with what was there before.
type Replaced = (PathBuf, Vec<(usize, Option<Frame>)>);

/// What the agent has changed that an open page does not know about.
///
/// The page holds its edits in memory and writes them on `save`. An agent
/// writing the same track from underneath is the one way this app can lose
/// work, so the server counts its own writes and the page watches the count.
/// It cannot merge — nothing here could — but it can refuse to be silent,
/// which is the difference between a conflict and a disappearance.
#[derive(Debug, Default)]
pub struct Desk {
    pub revision: Mutex<u64>,
    /// What the last proposals replaced, newest last. In memory, never on
    /// disk, and the same rule the page's own undo follows: undo is an editing
    /// convenience and the record is the ticks.
    pub undo: Mutex<Vec<Replaced>>,
}

impl Desk {
    fn bump(&self) -> u64 {
        let mut r = self.revision.lock().unwrap();
        *r += 1;
        *r
    }
}

/// Reads the track a path holds, or an empty one under that name.
pub(crate) fn read(track: &Path, skeleton: &Skeleton) -> Track {
    metal_vfs::read_to_string(track)
        .ok()
        .and_then(|t| serde_json::from_str::<Track>(&t).ok())
        .unwrap_or_else(|| Track {
            name: track
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("untitled")
                .split('.')
                .next()
                .unwrap_or("untitled")
                .to_owned(),
            rig: skeleton.name.clone(),
            joints: skeleton.len(),
            rate: metal_rig::motion::KEY_RATE,
            frames: Vec::new(),
        })
}

fn write(track: &Track, to: &Path) -> Result<(), String> {
    track.validate().map_err(|e| e.to_string())?;
    if let Some(dir) = to.parent() {
        metal_vfs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let json = serde_json::to_string_pretty(track).map_err(|e| e.to_string())?;
    crate::atomic::write(to, json).map_err(|e| e.to_string())
}

/// One node's line in a proposal. `None` is silence: take the statement back.
struct Ask {
    node: NodeId,
    turn: Option<Vec3>,
}

/// Reads `{"id": 27, "turn": [x, y, z]}` or `{"id": 27, "silent": true}`.
///
/// Every refusal here is a 400 rather than a silent correction, because a
/// proposal the agent got wrong is a proposal the operator has to see refused — the
/// alternative is a day of captures of something nobody asked for.
fn ask(v: &Value, nodes: &Nodes) -> Result<Ask, String> {
    let node = v
        .get("id")
        .and_then(Value::as_i64)
        .ok_or("a node needs an id")? as NodeId;
    // Zero is not a node, and this is the same refusal `Track::validate` makes
    // — made earlier so the message names the field instead of the tick.
    if node == 0 {
        return Err("0 is not a node; ids are negative on the axis, even right, odd left".into());
    }
    if nodes.joint(node).is_none() {
        return Err(format!("{node} is not a node in this rig"));
    }
    if v.get("silent").and_then(Value::as_bool) == Some(true) {
        return Ok(Ask { node, turn: None });
    }
    let a = v
        .get("turn")
        .and_then(Value::as_array)
        .ok_or_else(|| format!("node {node} needs `turn`: three numbers, a rotation vector"))?;
    if a.len() != 3 {
        return Err(format!("node {node}'s `turn` is three numbers"));
    }
    let n = |i: usize| {
        a[i].as_f64()
            .ok_or_else(|| format!("node {node}'s `turn` is numbers"))
    };
    let turn = Vec3::new(n(0)? as f32, n(1)? as f32, n(2)? as f32);
    // The encoding invariant, refused at the door so the message can name the
    // node. `Track::validate` refuses it again on the way to disk.
    if turn.length() > std::f32::consts::PI + 1e-4 {
        return Err(format!(
            "node {node} turns {:.2} rad; past π is the long way round to somewhere π reaches",
            turn.length()
        ));
    }
    Ok(Ask {
        node,
        turn: Some(turn),
    })
}

/// Applies one tick's worth of asks onto a frame.
///
/// There is nothing to reconcile between them any more. Two asks for one node
/// used to be a contradiction worth refusing — *given and carried* named two
/// different mechanisms and nobody could tell which was meant — and now the
/// last one simply wins, the way writing to the same key twice does.
fn lay(frame: &mut Frame, asks: &[Ask]) {
    for a in asks {
        match a.turn {
            // Silence is not rest: a proposal that silences a node removes the
            // statement and writes nothing in its place.
            None => {
                frame.turns.remove(&a.node);
            }
            Some(v) => {
                frame.turns.insert(a.node, v);
            }
        }
    }
}

/// `POST /proposal`. Applies and answers with what the body now does.
///
/// Takes a single tick — a pose — or a range of them — a movement. the operator's
/// own distinction, and the reason the endpoint takes both rather than making
/// the agent send a movement one request at a time.
pub fn proposal(
    body: &[u8],
    track_path: &Path,
    skeleton: &Skeleton,
    nodes: &Nodes,
    desk: &Desk,
) -> Result<Value, String> {
    let v: Value = serde_json::from_slice(body).map_err(|e| e.to_string())?;
    let from = v
        .get("tick")
        .or_else(|| v.get("from"))
        .and_then(Value::as_u64)
        .ok_or("a proposal needs a tick")? as usize;

    // One tick, or a range of them.
    let per_tick: Vec<Vec<Value>> = match v.get("ticks").and_then(Value::as_array) {
        Some(list) => list
            .iter()
            .map(|t| {
                t.get("nodes")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default()
            })
            .collect(),
        None => vec![v
            .get("nodes")
            .and_then(Value::as_array)
            .cloned()
            .ok_or("a proposal needs `nodes`")?],
    };

    let mut track = read(track_path, skeleton);
    let mut replaced: Vec<(usize, Option<Frame>)> = Vec::new();
    for (i, asks) in per_tick.iter().enumerate() {
        let tick = from + i;
        let asks: Vec<Ask> = asks
            .iter()
            .map(|a| ask(a, nodes))
            .collect::<Result<_, _>>()?;
        while track.frames.len() <= tick {
            track.frames.push(Frame::default());
        }
        replaced.push((tick, Some(track.frames[tick].clone())));
        lay(&mut track.frames[tick], &asks);
    }
    // Through the real type before anything lands, which is the whole reason
    // the agent goes through this door rather than writing the file itself.
    write(&track, track_path)?;
    desk.undo
        .lock()
        .unwrap()
        .push((track_path.to_owned(), replaced));
    let revision = desk.bump();

    let touched: Vec<usize> = (from..from + per_tick.len()).collect();
    Ok(json!({
        "ok": true,
        "revision": revision,
        "track": track_path.display().to_string(),
        "ticks": touched.iter().map(|&t| state(&track, t, skeleton, nodes)).collect::<Vec<_>>(),
    }))
}

/// `POST /undo`. Puts back what the last proposal replaced.
///
/// `the design notes` §4: every edit is undoable, and undo is tested, not assumed. A
/// proposal is an edit, so it is on a stack — in memory, like the page's, and
/// for the same reason.
pub fn undo(skeleton: &Skeleton, desk: &Desk) -> Result<Value, String> {
    let Some((path, replaced)) = desk.undo.lock().unwrap().pop() else {
        return Err("nothing to undo".into());
    };
    let mut track = read(&path, skeleton);
    for (tick, was) in replaced.into_iter().rev() {
        match was {
            Some(frame) if tick < track.frames.len() => track.frames[tick] = frame,
            Some(frame) => {
                while track.frames.len() < tick {
                    track.frames.push(Frame::default());
                }
                track.frames.push(frame);
            }
            None => {
                if tick < track.frames.len() {
                    track.frames[tick] = Frame::default();
                }
            }
        }
    }
    write(&track, &path)?;
    Ok(json!({ "ok": true, "revision": desk.bump(), "track": path.display().to_string() }))
}

/// What one tick says, and where the body actually is when it has been said.
///
/// Both, because they are different questions and the second is the one worth
/// capturing. `turn` is the declaration — a rotation — and `at` is the world
/// place the node ends up in, which needs the replay.
pub fn state(track: &Track, tick: usize, skeleton: &Skeleton, nodes: &Nodes) -> Value {
    let mut pose = Figure::Standing.pose(skeleton);
    track.pose_through(nodes, tick, &mut pose);
    let world = pose.world(skeleton);
    let points: Vec<Vec3> = world.iter().map(|m| m.translation.into()).collect();
    // The world *orientations*, one quaternion per joint index. The shell's
    // turn gesture (`S.U9c`'s `aim`) needs the parent's world frame to
    // express a drag as a local rotation vector — the same `w.rot` the page
    // kept in `rig.js`'s `world()`, and the reason it is exposed here rather
    // than re-derived in the panel (the one-geometry rule this payload's
    // `points` comment already states).
    let rots: Vec<[f32; 4]> = world
        .iter()
        .map(|m| {
            let q = m.to_scale_rotation_translation().1;
            [q.x, q.y, q.z, q.w]
        })
        .collect();

    let empty = Frame::default();
    let frame = track.frames.get(tick).unwrap_or(&empty);
    let mut said: BTreeMap<String, Value> = BTreeMap::new();
    for (node, v) in &frame.turns {
        let Some(joint) = nodes.joint(*node) else {
            continue;
        };
        let p = points[joint as usize];
        said.insert(
            node.to_string(),
            json!({
                "name": skeleton.joints[joint as usize].name,
                "turn": [v.x, v.y, v.z],
                "at": [p.x, p.y, p.z],
            }),
        );
    }

    // `Issue::MovesNothing`, answered before it costs a day. A joint's rotation
    // places its children, so a rotation on a joint that has none is a
    // statement with no geometry in it — invisible to an agent that cannot see
    // the body, and the failure a rotation notation actually hits. It replaces
    // `nothing_moves_it`, which reported the same class of mistake against the
    // chain solver that no longer exists.
    let stuck: Vec<Value> = frame
        .turns
        .keys()
        .filter_map(|node| nodes.joint(*node).map(|j| (*node, j)))
        .filter(|(_, j)| skeleton.is_terminal(*j))
        .map(|(node, j)| {
            json!({
                "node": node,
                "name": skeleton.joints[j as usize].name,
                "why": "nothing hangs off it, so turning it moves no point on the body",
            })
        })
        .collect();

    let (written, unwritten) = frame.counts(skeleton.len());
    json!({
        "tick": tick,
        "seconds": tick as f64 / track.rate,
        "ticks": track.frames.len(),
        // S.U9c (15-sept-2026): the posed figure in full — one point per
        // joint index, the same order/numbering every consumer keys off.
        // Nobody re-runs forward kinematics of its own, the rule
        // author/rig.rs's payload comment states ("two implementations of
        // the rig's geometry disagreeing by a millimetre is a whole corpus
        // authored a millimetre off").
        "points": points
            .iter()
            .map(|p| json!([p.x, p.y, p.z]))
            .collect::<Vec<_>>(),
        // World orientation per joint, `[x, y, z, w]` — the frame a turn is
        // expressed in; most consumers ignore it.
        "rot": rots
            .iter()
            .map(|q| json!([q[0], q[1], q[2], q[3]]))
            .collect::<Vec<_>>(),
        "nodes": said,
        // Silence is the rest of the body and is deliberately not listed: a
        // proposal that mentioned every silent node would be a proposal that
        // asserted thirty-odd things nobody looked at.
        "counts": { "written": written, "silent": unwritten },
        "travel": frame.travel.map(|t| json!([t.x, t.y, t.z])),
        "moves_nothing": stuck,
    })
}

/// `GET /state?tick=N`.
pub fn state_at(
    track_path: &Path,
    tick: usize,
    skeleton: &Skeleton,
    nodes: &Nodes,
    desk: &Desk,
) -> Value {
    let track = read(track_path, skeleton);
    let mut out = state(&track, tick, skeleton, nodes);
    out["revision"] = json!(*desk.revision.lock().unwrap());
    out["track"] = json!(track_path.display().to_string());
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use metal_rig::rig;

    fn bench(name: &str) -> (Skeleton, Nodes, PathBuf, Desk) {
        let s = rig::canonical();
        let n = Nodes::of(&s);
        let dir = std::env::temp_dir().join(format!("metal-agent-{name}"));
        let _ = metal_vfs::remove_dir_all(&dir);
        metal_vfs::create_dir_all(&dir).unwrap();
        (s, n, dir.join("t.track.json"), Desk::default())
    }

    fn node(s: &Skeleton, n: &Nodes, name: &str) -> NodeId {
        n.id(s.find(name).unwrap())
    }

    /// The instruction from the handoff, in the notation it is written in now:
    /// *turn l_shoulder to [0, 0, 1.2] at tick 42*. Under the old notation this
    /// same instruction needed a second line carrying the clavicle, and got it
    /// wrong — see `a_declaration_nothing_can_move_is_reported`, deleted with
    /// the mechanism that made it necessary.
    #[test]
    fn a_proposal_applies_and_reads_back_as_what_was_asked() {
        let (s, n, path, desk) = bench("apply");
        let shoulder = node(&s, &n, "l_shoulder");
        let elbow = node(&s, &n, "l_elbow");
        let body = json!({
            "tick": 42,
            "nodes": [
                { "id": shoulder, "turn": [0.0, 0.0, 1.2] },
                { "id": elbow, "turn": [0.0, 0.6, 0.0] },
            ]
        });
        let out = proposal(body.to_string().as_bytes(), &path, &s, &n, &desk).expect("applied");
        assert_eq!(out["ok"], true);
        let tick = &out["ticks"][0];
        assert_eq!(tick["tick"], 42);
        assert!(
            (tick["nodes"][shoulder.to_string()]["turn"][2]
                .as_f64()
                .unwrap()
                - 1.2)
                .abs()
                < 1e-6
        );
        assert_eq!(tick["counts"]["written"], 2);
        assert_eq!(tick["counts"]["silent"], s.len() - 2);

        // And what landed on disk is a Track the workspace reads, which is the
        // whole reason the agent goes through this door.
        let back: Track = serde_json::from_str(&metal_vfs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            back.frames.len(),
            43,
            "the ticks before it exist and are empty"
        );
        assert!(back.frames[41].is_empty());
        assert_eq!(back.frames[42].counts(s.len()), (2, s.len() - 2));
        assert!(back.validate().is_ok());
    }

    /// `turn` is a rotation and the state says where the body actually ended
    /// up — which is the number a verification needs and the one the proposal
    /// does not contain.
    #[test]
    fn the_state_answers_both_what_was_said_and_where_the_body_is() {
        let (s, n, path, desk) = bench("state");
        let shoulder = node(&s, &n, "l_shoulder");
        proposal(
            json!({ "tick": 0, "nodes": [{ "id": shoulder, "turn": [0.0, 0.0, 1.3] }] })
                .to_string()
                .as_bytes(),
            &path,
            &s,
            &n,
            &desk,
        )
        .unwrap();

        let got = state_at(&path, 0, &s, &n, &desk);
        let said = &got["nodes"][shoulder.to_string()];
        // f32 through JSON, so compare as a number rather than as a literal.
        assert!(
            (said["turn"][2].as_f64().unwrap() - 1.3).abs() < 1e-6,
            "the rotation, as asked"
        );
        let at: Vec<f64> = said["at"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_f64().unwrap())
            .collect();
        let standing = Figure::Standing.pose(&s).points(&s);
        let elbow = s.find("l_elbow").unwrap() as usize;
        assert_eq!(at.len(), 3, "the shoulder's own place is reported");
        // The arm moved: the elbow under it is nowhere near where it hung.
        let mut pose = Figure::Standing.pose(&s);
        read(&path, &s).pose_through(&n, 0, &mut pose);
        assert!(pose.points(&s)[elbow].distance(standing[elbow]) > 0.2);
    }

    /// The failure an agent that cannot see the body will actually hit under a
    /// rotation notation: a joint's rotation places its children, so writing
    /// one on a fingertip is a statement with no geometry in it. Invisible, so
    /// the state says so out loud.
    #[test]
    fn a_rotation_that_moves_nothing_is_reported_rather_than_ignored() {
        let (s, n, path, desk) = bench("stuck");
        let tip = node(&s, &n, "l_finger_tip");
        let out = proposal(
            json!({ "tick": 0, "nodes": [{ "id": tip, "turn": [0.0, 0.4, 0.0] }]})
                .to_string()
                .as_bytes(),
            &path,
            &s,
            &n,
            &desk,
        )
        .unwrap();
        let stuck = out["ticks"][0]["moves_nothing"].as_array().unwrap();
        assert_eq!(stuck.len(), 1, "{stuck:?}");
        assert_eq!(stuck[0]["name"], "l_finger_tip");

        // Turn the knuckles instead and the fingers actually gather.
        let fingers = node(&s, &n, "l_fingers");
        let out = proposal(
            json!({ "tick": 1, "nodes": [{ "id": fingers, "turn": [0.0, 0.0, -1.0] }]})
                .to_string()
                .as_bytes(),
            &path,
            &s,
            &n,
            &desk,
        )
        .unwrap();
        assert!(out["ticks"][0]["moves_nothing"]
            .as_array()
            .unwrap()
            .is_empty());
    }

    /// A range is a movement; one tick is a pose. the operator's own distinction, and
    /// the reason the endpoint takes both.
    #[test]
    fn a_proposal_can_be_a_range_of_ticks() {
        let (s, n, path, desk) = bench("range");
        let elbow = node(&s, &n, "l_elbow");
        let step = |y: f64| json!({ "nodes": [{ "id": elbow, "turn": [0.0, y, 0.0] }] });
        let out = proposal(
            json!({ "from": 5, "ticks": [step(0.3), step(0.6), step(0.9)] })
                .to_string()
                .as_bytes(),
            &path,
            &s,
            &n,
            &desk,
        )
        .unwrap();
        assert_eq!(out["ticks"].as_array().unwrap().len(), 3);
        assert_eq!(out["ticks"][0]["tick"], 5);
        assert_eq!(out["ticks"][2]["tick"], 7);
        let back: Track = serde_json::from_str(&metal_vfs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(back.frames.len(), 8);
        assert_eq!(back.frames[6].counts(s.len()).0, 1);
    }

    /// Zero is not a node, and the proposal door refuses it the same way the
    /// save door does — earlier, and with the field named.
    #[test]
    fn a_zero_keyed_proposal_is_refused_and_nothing_is_written() {
        let (s, n, path, desk) = bench("zero");
        let body = json!({ "tick": 0, "nodes": [{ "id": 0, "turn": [0.1, 0.0, 0.0] }] });
        let err = proposal(body.to_string().as_bytes(), &path, &s, &n, &desk).unwrap_err();
        assert!(err.contains("0 is not a node"), "{err}");
        assert!(!metal_vfs::exists(&path), "a refused proposal must not reach the disk");

        // And a node this rig does not have.
        let body = json!({ "tick": 0, "nodes": [{ "id": 9999, "turn": [0.1, 0.0, 0.0] }] });
        assert!(proposal(body.to_string().as_bytes(), &path, &s, &n, &desk).is_err());
        assert!(!metal_vfs::exists(&path));
    }

    /// The encoding invariant, refused at the door with the node named rather
    /// than at the disk with the tick named.
    #[test]
    fn a_rotation_past_pi_is_refused() {
        let (s, n, path, desk) = bench("long");
        let elbow = node(&s, &n, "l_elbow");
        let body = json!({ "tick": 0, "nodes": [{ "id": elbow, "turn": [0.0, 4.0, 0.0] }] });
        let err = proposal(body.to_string().as_bytes(), &path, &s, &n, &desk).unwrap_err();
        assert!(err.contains("long way round"), "{err}");
        assert!(!metal_vfs::exists(&path));
    }

    /// A node line is a rotation or a silence, and nothing else. The old form
    /// carried a `role`, and a proposal still sending one has to fail loudly
    /// rather than write an unturned joint.
    #[test]
    fn a_node_line_without_a_turn_is_refused() {
        let (s, n, path, desk) = bench("noturn");
        let w = node(&s, &n, "l_wrist");
        for body in [
            json!({ "tick": 0, "nodes": [{ "id": w }] }),
            json!({ "tick": 0, "nodes": [{ "id": w, "role": "carried" }] }),
            json!({ "tick": 0, "nodes": [{ "id": w, "turn": [0.1, 0.0] }] }),
        ] {
            let err = proposal(body.to_string().as_bytes(), &path, &s, &n, &desk).unwrap_err();
            assert!(err.contains("turn"), "{err}");
        }
        assert!(!metal_vfs::exists(&path));
    }

    /// A zero rotation is a statement — the joint is at its rest rotation —
    /// where it used to be canonicalised into the reset. Silence is the
    /// different thing, and it removes the statement instead.
    #[test]
    fn a_zero_rotation_is_written_and_a_silence_takes_it_back() {
        let (s, n, path, desk) = bench("zero_turn");
        let w = node(&s, &n, "l_wrist");
        proposal(
            json!({ "tick": 0, "nodes": [{ "id": w, "turn": [0.0, 0.0, 0.0] }]})
                .to_string()
                .as_bytes(),
            &path,
            &s,
            &n,
            &desk,
        )
        .unwrap();
        let back = read(&path, &s);
        assert_eq!(
            back.frames[0].get(w),
            Some(Vec3::ZERO),
            "the zero is in the record"
        );
        assert_eq!(
            back.frames[0].write(),
            format!("[{w}, 0.0000, 0.0000, 0.0000]")
        );

        proposal(
            json!({ "tick": 0, "nodes": [{ "id": w, "silent": true }]})
                .to_string()
                .as_bytes(),
            &path,
            &s,
            &n,
            &desk,
        )
        .unwrap();
        let back = read(&path, &s);
        assert_eq!(back.frames[0].get(w), None, "the statement is gone");
        assert!(back.frames[0].is_empty());
    }

    /// Silence is not rest, and the nodes a proposal never mentions are not
    /// touched at all.
    #[test]
    fn a_proposal_only_touches_the_nodes_it_mentions() {
        let (s, n, path, desk) = bench("silence");
        let (a, b) = (node(&s, &n, "l_wrist"), node(&s, &n, "r_wrist"));
        proposal(
            json!({ "tick": 0, "nodes": [
                { "id": a, "turn": [0.1, 0.0, 0.0] },
                { "id": b, "turn": [0.2, 0.0, 0.0] }]})
            .to_string()
            .as_bytes(),
            &path,
            &s,
            &n,
            &desk,
        )
        .unwrap();
        proposal(
            json!({ "tick": 0, "nodes": [{ "id": a, "silent": true }]})
                .to_string()
                .as_bytes(),
            &path,
            &s,
            &n,
            &desk,
        )
        .unwrap();

        let back = read(&path, &s);
        assert_eq!(back.frames[0].get(a), None, "the statement is gone");
        assert_eq!(
            back.frames[0].get(b),
            Some(Vec3::new(0.2, 0.0, 0.0)),
            "the node it did not mention held"
        );
    }

    /// `the design notes` §4: every edit is undoable, and undo is tested, not assumed.
    #[test]
    fn a_proposal_is_undoable() {
        let (s, n, path, desk) = bench("undo");
        let w = node(&s, &n, "l_wrist");
        let put = |by: f64| {
            proposal(
                json!({ "tick": 3, "nodes": [{ "id": w, "turn": [by, 0.0, 0.0] }]})
                    .to_string()
                    .as_bytes(),
                &path,
                &s,
                &n,
                &desk,
            )
            .unwrap();
        };
        put(0.1);
        put(0.2);
        let read_back = || read(&path, &s);
        assert_eq!(read_back().frames[3].get(w), Some(Vec3::new(0.2, 0.0, 0.0)));

        undo(&s, &desk).unwrap();
        assert_eq!(
            read_back().frames[3].get(w),
            Some(Vec3::new(0.1, 0.0, 0.0)),
            "one step back, not all the way"
        );
        undo(&s, &desk).unwrap();
        assert!(read_back().frames[3].is_empty(), "and back to nothing said");
        assert!(undo(&s, &desk).is_err(), "it runs out rather than throwing");
    }

    /// The counter an open page watches, so an agent writing underneath it is a
    /// conflict rather than a disappearance.
    #[test]
    fn every_write_moves_the_revision() {
        let (s, n, path, desk) = bench("rev");
        let w = node(&s, &n, "l_wrist");
        let before = *desk.revision.lock().unwrap();
        proposal(
            json!({ "tick": 0, "nodes": [{ "id": w, "turn": [0.1, 0.0, 0.0] }]})
                .to_string()
                .as_bytes(),
            &path,
            &s,
            &n,
            &desk,
        )
        .unwrap();
        let after = *desk.revision.lock().unwrap();
        assert!(after > before);
        assert_eq!(state_at(&path, 0, &s, &n, &desk)["revision"], after);
        undo(&s, &desk).unwrap();
        assert!(
            *desk.revision.lock().unwrap() > after,
            "an undo is a write too"
        );
    }
}
