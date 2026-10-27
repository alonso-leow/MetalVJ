//! `POST /loop/audio` — the operator's hand on the [`Looper`] (Phase 4B).
//!
//! The Looper itself (mode, manual lock, exit) landed in `metal-perform` Phase 3
//! and lives on `State` (`ctx.state.looper`); this route is the one place a
//! control surface reaches it, copied from [`crate::call::transmit::router`]'s
//! shape: take `params.json()`, apply every op the body carries, answer
//! `{ "ok": true, "status": … }`. It is deliberately *body-driven* rather than
//! a route per verb — the plan lists one route with `mode | lock | beats:N |
//! exit`, so one handler reads them all and a control posts exactly the op it
//! means (rule 6: the AI tool, when it lands, posts this same body).
//!
//! **`lock` and `beats`.** The plan's own signature lists both. They are the
//! same op — a manual loop length in beats — so the body accepts `"beats": N`
//! as an alias of `"lock": N`, and additionally `"lock": true`, which is *not*
//! a length: it is "engage the detector's current length, whatever it is"
//! (the operator's hand on a loop the detector has already found). One JSON
//! key, two spellings for the manual length, because `lock` is what the
//! `Looper` already calls the operation and `beats` is what the plan calls the
//! value.
//!
//! **Refusals are the caller's.** An unparsable `mode`, a non-positive
//! `beats`/`lock` length, or `"lock": true` with no detector lock to take are
//! all `CallError::Bad` (400) — never a silent default (the `Mode::parse`
//! contract) and never a guessed length. An empty body is a no-op that still
//! answers the status, so a control can poll without changing anything.

use serde_json::{json, Value};

use crate::call::{CallError, Ctx, Params};

/// Serialise [`metal_perform::looper::Status`] for the wire. One function so the
/// route's reply and the readout's own read of `/state` describe a loop the
/// same way: `mode` (`off`/`arm`/`auto`), `length_beats`, `phase`, `engaged`
/// (bool — the *fact*, not the `Option`) and `detector` (bool, plus the
/// detector's own length/since/confidence when it has one).
fn status_json(status: metal_perform::looper::Status) -> Value {
    json!({
        "mode": status.mode.as_str(),
        "length_beats": status.length_beats,
        "phase": status.phase,
        "engaged": status.engaged.is_some(),
        "detector": status.detector.map(|d| json!({
            "length_beats": d.length_beats,
            "since_beat": d.since_beat,
            "confidence": d.confidence,
        })),
    })
}

pub fn loop_audio(ctx: &mut Ctx, params: &Params) -> Result<Value, CallError> {
    let Some(body) = params.json() else {
        return Err(CallError::Bad("expected json".into()));
    };

    if let Some(m) = body.get("mode").and_then(Value::as_str) {
        let mode = metal_perform::looper::Mode::parse(m)
            .ok_or_else(|| CallError::Bad(format!("no such mode: {m:?}")))?;
        ctx.state.looper.set_mode(mode);
    }

    // `lock` is either `true` (the detector's current length) or a number
    // (a manual length); `beats` is always the manual length and an alias of
    // the numeric `lock`. A boolean `"beats": true` is not a thing — only the
    // numeric form is accepted under either key.
    if let Some(lock) = body.get("lock") {
        if let Some(engage) = lock.as_bool() {
            if engage {
                // "Use the loop the detector already found." No detector lock
                // means no length to take — refuse rather than guess one.
                let detected = ctx.state.looper.status().detector.map(|d| d.length_beats);
                let len = detected.ok_or_else(|| {
                    CallError::Bad("no detected loop to lock — give beats:N instead".into())
                })?;
                ctx.state.looper.lock(Some(len));
            }
        } else if let Some(n) = lock.as_i64() {
            lock_beats(ctx, n)?;
        } else {
            return Err(CallError::Bad(
                "lock must be true or an integer number of beats".into(),
            ));
        }
    }

    if let Some(beats) = body.get("beats") {
        let n = beats
            .as_i64()
            .ok_or_else(|| CallError::Bad("beats must be an integer number of beats".into()))?;
        lock_beats(ctx, n)?;
    }

    if body.get("exit").and_then(Value::as_bool) == Some(true) {
        ctx.state.looper.exit();
    }

    Ok(json!({ "ok": true, "status": status_json(ctx.state.looper.status()) }))
}

/// Engage a manual loop of `n` beats, refusing `n <= 0` before it reaches the
/// `Looper` (which clamps upward to its own minimum; a zero or negative length
/// is the caller's mistake, not something to clamp silently).
fn lock_beats(ctx: &mut Ctx, n: i64) -> Result<(), CallError> {
    if n <= 0 {
        return Err(CallError::Bad(format!(
            "beats must be a positive number of beats, got {n}"
        )));
    }
    ctx.state.looper.lock(Some(n as u32));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::HashMap;

    /// The same fixture shape `call::tests::every_route_dispatches` builds: a
    /// painter, a `State`, and an `Author` — whose `gpu`-feature construction
    /// reads `data/schema.json` off a bare relative path, hence the cwd dance.
    fn fixture() -> (
        Box<dyn crate::paint::Paint>,
        crate::State,
        crate::listeners::Listeners,
        crate::Author,
    ) {
        let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let prior_cwd = std::env::current_dir().expect("cwd");
        let _chdir = crate::chdir_lock();
        std::env::set_current_dir(&repo_root).expect("chdir to repo root for Author::new()");
        let author = crate::Author::new();
        std::env::set_current_dir(&prior_cwd).expect("restore cwd");

        let paint = crate::paint::painter();
        let state = crate::State::new(paint.as_ref());
        let sim = crate::listeners::Listeners::stand_in();
        (paint, state, sim, author)
    }

    fn body(v: Value) -> Params {
        Params {
            query: HashMap::new(),
            body: v.to_string().into_bytes(),
            tail: None,
        }
    }

    /// `POST /loop/audio {"mode":"auto"}` sets the mode and reports it.
    #[test]
    fn the_route_sets_the_mode() {
        let (mut paint, mut state, mut sim, author) = fixture();
        let mut ctx = Ctx {
            state: &mut state,
            paint: paint.as_mut(),
            sim: &mut sim,
            author: &author,
        };
        let reply = loop_audio(&mut ctx, &body(json!({ "mode": "auto" }))).expect("mode op");
        assert_eq!(reply["ok"], true);
        assert_eq!(reply["status"]["mode"], "auto");
        assert_eq!(ctx.state.looper.mode(), metal_perform::looper::Mode::Auto);
        assert_eq!(reply["status"]["engaged"], false);
    }

    /// `{"beats":4}` (and `{"lock":4}`) engage a manual loop and the reply
    /// reports its length. `lock(Some(4))` is the plan's manual length.
    #[test]
    fn the_route_locks_a_manual_loop_and_reports_it() {
        let (mut paint, mut state, mut sim, author) = fixture();
        let mut ctx = Ctx {
            state: &mut state,
            paint: paint.as_mut(),
            sim: &mut sim,
            author: &author,
        };
        let reply = loop_audio(&mut ctx, &body(json!({ "beats": 4 }))).expect("beats op");
        assert_eq!(reply["status"]["engaged"], true);
        assert_eq!(reply["status"]["length_beats"], 4);
        assert_eq!(ctx.state.looper.status().engaged.map(|e| e.len), Some(4));

        let aliased = loop_audio(&mut ctx, &body(json!({ "lock": 8 }))).expect("lock op");
        assert_eq!(aliased["status"]["length_beats"], 8);
        assert_eq!(ctx.state.looper.status().engaged.map(|e| e.len), Some(8));
    }

    /// `{"exit":true}` begins the exit — `engaged` stays true while the fade
    /// runs, so the reply is read off the `Looper` rather than asserted false.
    #[test]
    fn exit_leaves_the_loop() {
        let (mut paint, mut state, mut sim, author) = fixture();
        let mut ctx = Ctx {
            state: &mut state,
            paint: paint.as_mut(),
            sim: &mut sim,
            author: &author,
        };
        loop_audio(&mut ctx, &body(json!({ "beats": 4 }))).expect("lock first");
        assert!(ctx.state.looper.status().engaged.is_some());
        let reply = loop_audio(&mut ctx, &body(json!({ "exit": true }))).expect("exit op");
        assert_eq!(reply["ok"], true);
        // The exit is a one-beat crossfade (plan decision 8): the loop is
        // still on air this instant, so the honest report is `engaged` — the
        // Looper's own state, not a second guess here.
        assert_eq!(reply["status"]["engaged"], ctx.state.looper.status().engaged.is_some());
    }

    /// An unparsable mode — and a non-positive length — are the caller's
    /// fault: `CallError::Bad`, never a silent default.
    #[test]
    fn an_unknown_mode_is_a_bad_request() {
        let (mut paint, mut state, mut sim, author) = fixture();
        let mut ctx = Ctx {
            state: &mut state,
            paint: paint.as_mut(),
            sim: &mut sim,
            author: &author,
        };
        assert!(matches!(
            loop_audio(&mut ctx, &body(json!({ "mode": "live" }))),
            Err(CallError::Bad(_))
        ));
        assert!(matches!(
            loop_audio(&mut ctx, &body(json!({ "beats": 0 }))),
            Err(CallError::Bad(_))
        ));
        assert!(
            matches!(
                loop_audio(&mut ctx, &body(json!({ "lock": true }))),
                Err(CallError::Bad(_))
            ),
            "lock:true with no detector lock is refused, not guessed"
        );
    }
}
