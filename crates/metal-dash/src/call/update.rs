//! `GET`/`POST /update` — the in-app updater's one route pair.
//!
//! Two operations, never one combined "update me" call:
//!
//! - **`GET /update`** reads the release list, picks this channel's newest,
//!   compares it to the running version, and answers. It **never** downloads
//!   or writes anything. A panel can poll it on a timer.
//! - **`POST /update`** with `{"op": "apply", "tag": "v1.4.1"}` is the only
//!   thing that downloads. It requires an explicit `tag` (the one a person saw
//!   on the `GET`), and refuses when the channel is `off`.
//!
//! The split is the "never auto-update mid-show" rule enforced in the shape of
//! the API rather than in a caller's good behaviour. See
//! [`crate::update`]'s own module docs for the flag and the config table.
//!
//! # The wire shape
//!
//! Both answer one stable JSON object. A client should switch on `state`:
//!
//! ```json
//! {
//!   "op": "check",
//!   "state": "available",
//!   "channel": "main",
//!   "repo": "alonso-leow/metalvj",
//!   "current": "1.4.0",
//!   "latest": {
//!     "tag": "v1.4.1",
//!     "version": "1.4.1",
//!     "published": "2026-09-01T08:30:00Z",
//!     "url": "https://github.com/alonso-leow/metalvj/releases/tag/v1.4.1",
//!     "notes": "Bug fixes.",
//!     "prerelease": false,
//!     "asset": {
//!       "name": "metalvj-1.4.1-windows-x86_64.exe",
//!       "url": "https://github.com/.../metalvj-1.4.1-windows-x86_64.exe",
//!       "size": 38000000,
//!       "digest": "sha256:…"
//!     }
//!   },
//!   "message": "1.4.1 is available"
//! }
//! ```
//!
//! `state` is one of:
//!
//! | `state` | meaning | `latest` | can `apply`? |
//! | --- | --- | --- | --- |
//! | `off` | `[update].channel = "off"` (or the build has no updater) | `null` | no |
//! | `up_to_date` | the channel's newest is not newer than this build | maybe `null` | no |
//! | `available` | a newer release exists (its asset may still be `null`) | object | yes, if `asset` |
//! | `ahead` | this build is newer than its own channel's newest tag | object | no |
//! | `error` | the check itself failed (network, rate limit, bad JSON) | `null` | no |
//! | `applied` | an `apply` ran: see `applied` below | object | — |
//! | `refused` | an `apply` was asked for something it will not do | object | — |
//!
//! An `applied` / `refused` answer adds:
//!
//! ```json
//! {
//!   "op": "apply",
//!   "state": "applied",
//!   "restart": true,
//!   "verified": true,
//!   "path": "C:\\Users\\dj\\.local\\bin\\metal-dash.exe",
//!   "pending": "C:\\Users\\dj\\.local\\bin\\metal-dash.exe.new",
//!   "message": "1.4.1 downloaded; restart MetalVJ to finish"
//! }
//! ```
//!
//! A refusal or a failed apply is a JSON body with `state: "error"` or
//! `state: "refused"` and a `message`, and the HTTP status carries the class
//! (400 for a bad body, 409 for "off"/wrong tag, 500 for a failed download) —
//! the same `message` a human reads and the same one the panel shows.

use serde_json::Value;

use crate::call::{CallError, Ctx, Params};

/// `GET /update`: read-only. The channel and the release list come from
/// `[update]` in `config.toml` (see `crate::update`).
pub fn get(_ctx: &mut Ctx, _params: &Params) -> Result<Value, CallError> {
    Ok(crate::update::check_reply())
}

/// `POST /update`: `{"op":"apply","tag":"v1.4.1"}` (or `{"op":"apply","version":
/// "1.4.1"}`). Anything but `apply` is a 400 — a route that guessed would be the
/// "auto-update mid-show" this crate exists to prevent.
pub fn post(_ctx: &mut Ctx, params: &Params) -> Result<Value, CallError> {
    let Some(body) = params.json() else {
        return Err(bad("expected json: {\"op\":\"apply\",\"tag\":\"<tag>\"}"));
    };
    match body.get("op").and_then(Value::as_str) {
        Some("apply") => {
            let tag = body
                .get("tag")
                .and_then(Value::as_str)
                .or_else(|| body.get("version").and_then(Value::as_str))
                .map(str::to_string);
            let Some(tag) = tag else {
                return Err(bad("apply needs the tag the check reported ({\"tag\":\"v1.4.1\"})"));
            };
            crate::update::apply_reply(&tag)
        }
        Some("check") => Ok(crate::update::check_reply()),
        Some(other) => Err(CallError::Bad(format!("unknown op: {other} (want \"apply\")"))),
        None => Err(bad("expected an op: {\"op\":\"apply\",\"tag\":\"<tag>\"}")),
    }
}

fn bad(message: impl Into<String>) -> CallError {
    CallError::Bad(message.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `Author::new()` reads `data/schema.json` off a relative path, so the
    /// cwd has to be the repo root while it is built — the same dance every
    /// route test in `call::tests` does (otherwise `Author::new` exits 1).
    fn fixture_author() -> crate::Author {
        use crate::chdir_lock;
        let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let prior = std::env::current_dir().expect("cwd");
        let _lock = chdir_lock();
        std::env::set_current_dir(&repo_root).expect("chdir to repo root");
        let author = crate::Author::new();
        std::env::set_current_dir(&prior).expect("restore cwd");
        author
    }

    /// A synthetic request, the same shape `call::tests` builds: the route
    /// answers its one JSON object without touching the network or the disk.
    fn reply(method: &str, body: &str) -> Result<Value, CallError> {
        let mut paint = crate::paint::painter();
        let mut state = crate::State::new(paint.as_ref());
        let mut sim = crate::listeners::Listeners::stand_in();
        let author = fixture_author();
        let params = Params { body: body.as_bytes().to_vec(), ..Params::default() };
        let mut ctx = Ctx { state: &mut state, paint: paint.as_mut(), sim: &mut sim, author: &author };
        match method {
            "GET" => get(&mut ctx, &params),
            _ => post(&mut ctx, &params),
        }
    }

    /// `GET` answers the stable object with a `state` from the documented set,
    /// and never downloads. Whichever of `off`/`up_to_date`/`available`/`ahead`/
    /// `error` this machine lands on (no network in a unit test), the keys a
    /// client reads are always there.
    #[test]
    fn get_answers_a_stable_shape_and_never_applies() {
        let v = reply("GET", "").expect("GET /update answers");
        let state = v.get("state").and_then(Value::as_str).expect("a state");
        assert!(
            ["off", "up_to_date", "available", "ahead", "error"].contains(&state),
            "unexpected state {state}: {v}"
        );
        for key in ["op", "state", "channel", "repo", "current", "message"] {
            assert!(v.get(key).is_some(), "{key} missing from {v}");
        }
        assert_eq!(v["op"], "check");
        assert!(!v.to_string().contains("applied"), "a GET never applies");
    }

    /// A body that is not an apply is refused, and the refusal names the shape.
    #[test]
    fn post_refuses_anything_but_an_explicit_apply() {
        let mut paint = crate::paint::painter();
        let mut state = crate::State::new(paint.as_ref());
        let mut sim = crate::listeners::Listeners::stand_in();
        let author = fixture_author();
        let mut ctx = Ctx { state: &mut state, paint: paint.as_mut(), sim: &mut sim, author: &author };
        let post_with = |ctx: &mut Ctx, body: &str| {
            let params = Params { body: body.as_bytes().to_vec(), ..Params::default() };
            post(ctx, &params)
        };
        assert!(matches!(post_with(&mut ctx, ""), Err(CallError::Bad(_))), "no body");
        assert!(
            matches!(post_with(&mut ctx, r#"{"op":"install"}"#), Err(CallError::Bad(_))),
            "an unknown op"
        );
        assert!(
            matches!(post_with(&mut ctx, r#"{"op":"apply"}"#), Err(CallError::Bad(_))),
            "apply without a tag"
        );
    }
}