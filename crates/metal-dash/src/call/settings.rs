//! The shell's own settings (`O.16`'s first slice, batch `S.AF`): the
//! dropdown that overrides `classes::arrangement`'s viewport decision
//! (`ShellMode`) — and, from here on, every shell-wide preference. One file
//! (`data/settings.json`), one route pair (`GET`/`POST /settings`), the
//! same trade `ai_config_path` made (`S.U14`): regla 6's "no second state
//! of the world" — the door is the one path in, the file is the one place
//! on disk, and a shell mode that survives a handoff to the tablet is the
//! same preference, not a second one (metal-sync already travels the other
//! `data/` jsons; `settings.json` joins them the day O.18 lands).
//!
//! The tight contract, modelled on `transmit::router`'s: a `POST` body
//! carries only keys this layer knows, an unknown key is dropped (the reply
//! shows the value never landed), and a value this layer does not accept is
//! refused with `Bad` rather than stored — a hand-edited
//! `data/settings.json` cannot leak a shape a panel would have to guess
//! against, because the *reply* is the only thing the panel ever reads.

use serde_json::{json, Map, Value};

use crate::call::{CallError, Ctx, Params};

/// The keys this layer owns. A new key means a new preference that grew this
/// slice (`O.16`'s naming/token sections, …) and added its own key *and* its
/// own tests here — never a second file.
const KEYS: [&str; 15] = [
    "shell_mode", "language", "tick_level", "grid_spans", "grid_page", "grid_controllers",
    "shell_view", "colour_rule", "export_dir", "fps_cap", "file_tags", "ingest_proxy",
    "ui_scale", "theme", "grid_surface",
];

pub use crate::frame_cap::{DEFAULT_FPS_CAP, FPS_CAPS};

#[cfg(test)]
/// The active fps cap: the file's on first use (adopted into `frame_cap`).
pub fn fps_cap() -> u32 {
    static LOADED: std::sync::Once = std::sync::Once::new();
    LOADED.call_once(|| {
        if let Some(n) = load_at(path()).get("fps_cap").and_then(Value::as_u64) {
            crate::frame_cap::set(n as u32);
        }
    });
    crate::frame_cap::get()
}

/// The active UI scale percentage: the file's on first use (adopted into
/// `ui_scale`).
pub fn ui_scale() -> u32 {
    static LOADED: std::sync::Once = std::sync::Once::new();
    LOADED.call_once(|| {
        if let Some(n) = load_at(path()).get("ui_scale").and_then(Value::as_u64) {
            crate::ui_scale::set(n as u32);
        }
    });
    crate::ui_scale::get()
}

/// The two views the header toggles between (`Edit` builds, `Show` plays):
/// spelled here so the route validates on every build, the same trade
/// [`LANGUAGES`] makes. A test ties it to `shell::ShellView`.
pub const SHELL_VIEWS: [&str; 2] = ["edit", "show"];

/// The UI languages the shell can paint in (`shell::i18n::Lang`'s codes),
/// spelled here so the route validates on every build, window or no window.
/// A test ties the two lists together.
pub const LANGUAGES: [&str; 2] = ["en", "es"];

/// `GET /settings`: whatever was on disk under the known keys, with the
/// defaults filled in wherever a key is absent — a fresh install (no file)
/// and a file that only carries keys it knows both answer the same default
/// shape, so the panel never has three cases.
pub fn get(_ctx: &mut Ctx, _params: &Params) -> Result<Value, CallError> {
    reply_at(&path()).map_err(CallError::Bad)
}

/// Applies a POST body key by key and answers the same full reply a GET
/// answers, so the panel repaints off the response without a follow-up
/// read (the same shape `ai_config_post_at` answers).
pub fn post(_ctx: &mut Ctx, params: &Params) -> Result<Value, CallError> {
    let Some(body) = params.json() else {
        return Err(CallError::Bad("expected json".into()));
    };
    post_at(&path(), &body).map_err(CallError::Bad)
}
/// The `grid-page-next` (`+1`) and `grid-page-prev` (`-1`) trigger targets:
/// steps the stored overlays page by `delta` and answers the new value. The
/// panel clamps it on its next frame, so a step past either end lands on the
/// first or last page the grid shows.
pub fn step_grid_page(delta: i64) -> Result<i64, String> {
    step_at(path(), delta)
}

fn step_at(path: impl AsRef<std::path::Path>, delta: i64) -> Result<i64, String> {
    let path = path.as_ref();
    let current = load_at(path).get("grid_page").and_then(Value::as_i64).unwrap_or(0);
    let next = current.saturating_add(delta);
    post_at(path, &json!({ "grid_page": next }))?;
    Ok(next)
}

/// Where the shell's own preference file lives — `data/`, alongside
/// `trims.json` and `ai.json`.
fn path() -> std::path::PathBuf {
    crate::output::data_root().join("settings.json")
}

/// The reply at `path`, with defaults where the disk is silent.
fn reply_at(path: impl AsRef<std::path::Path>) -> Result<Value, String> {
    Ok(Value::Object(load_at(path)))
}

/// The machine's active tick level, read off the one settings file the shell
/// route writes — the answer the agent's system prompt and any tick-addressed
/// reader states, so the notation is the active level and not a constant.
pub fn active_tick_level() -> metal_schema::TickRate {
    load_at(path())
        .get("tick_level")
        .and_then(Value::as_str)
        .and_then(metal_schema::TickRate::parse)
        .unwrap_or(metal_schema::TickRate::Base)
}

/// Read the file, then keep only the known keys and fill the known keys the
/// file is silent on with their defaults (`shell_mode: "auto"`). The
/// unknown keys a hand-edited file may carry are dropped here rather than
/// replayed in a reply the panel would have to ignore key by key.
fn load_at(path: impl AsRef<std::path::Path>) -> Map<String, Value> {
    let mut out = defaults();
    if let Some(obj) =
        serde_json::from_str::<Value>(&metal_vfs::read_to_string(path).unwrap_or_default())
            .ok()
            .and_then(|v| v.as_object().cloned())
    {
        for key in KEYS {
            if let Some(v) = obj.get(key) {
                out.insert((*key).into(), v.clone());
            }
        }
    }
    out
}

/// The fresh-install state: every key's default, spelled once. English is
/// the product language; Spanish is one pick away in the settings overlay.
fn defaults() -> Map<String, Value> {
    let mut m = Map::new();
    m.insert("shell_mode".into(), json!("auto"));
    m.insert("language".into(), json!("en"));
    // The minimum a fresh install can always afford; `fine` is one pick away.
    // The vocabulary is `metal_schema::TickRate`'s, so the route and the digester
    // cannot disagree about what "base" means (the voice design notes § "Two complete
    // levels, one switch").
    m.insert("tick_level".into(), json!(metal_schema::TickRate::Base.wire()));
    // The overlays grid's cell sizes, `{ "<key>": [w, h] }`, one of
    // `surface::SPANS` (1×1, 2×1, 1×2, 2×2, 1×0.5, 1×0.3, 0.5×1, 0.3×1).
    // No entry means 1×1, so a fresh install has no sizes to store.
    m.insert("grid_spans".into(), json!({}));
    // The overlays grid's page, 0-based. Signed and unclamped on purpose: the
    // `grid-page-next`/`-prev` trigger targets step it blind, and the panel
    // clamps it to the page count it lays out (it knows the count, not here).
    m.insert("grid_page".into(), json!(0));
    // The overlays grid's controller assignments, `{ "<key>": { "graph",
    // "name", "targets": [{ "node", … }] } }`. A fresh install assigns
    // nothing, so the map is empty; an empty object clears every controller.
    m.insert("grid_controllers".into(), json!({}));
    // The control surface (`surface::Layout`): the grid's fixed columns ×
    // rows, the cells' order and pages, the control cells, the MIDI bindings
    // and the recorded loops. A fresh install is an 8 × 4 grid, nothing bound.
    m.insert("grid_surface".into(), crate::surface::Layout::default().to_value());
    // The header's Edit|Show toggle; a fresh install opens on the editor.
    m.insert("shell_view".into(), json!("edit"));
    // Where the overlays grid's colour built-ins paint: a group name
    // (`all`, `models`, `effects`, `text`) or an array of node ids.
    m.insert("colour_rule".into(), json!("all"));
    // P-G: where "Export frame" writes; empty means `~/Pictures/MetalVJ`.
    m.insert("export_dir".into(), json!(""));
    // The global frame-rate cap (client, previews, output), 60 by default.
    m.insert("fps_cap".into(), json!(DEFAULT_FPS_CAP));
    // The global UI scale, a percentage (75..=200).
    m.insert("ui_scale".into(), json!(crate::ui_scale::DEFAULT));
    // The biblioteca's colour tags: `{ "<file or folder path>": <index> }`,
    // each index into `Theme::tag_colours`'s eight-preset ramp (`0..=7`). A
    // fresh install tags nothing, so the map is empty.
    m.insert("file_tags".into(), json!({}));
    // Whether a dropped clip builds its HAP proxy automatically; on by
    // default, so the editor shows a picture without a first preview.
    m.insert("ingest_proxy".into(), json!(true));
    // The client's colour theme: a stored palette's name (`Palettes`), or ""
    // for the house phosphor look.
    m.insert("theme".into(), json!(""));
    m
}

/// The folder `POST /frame/export` writes into: the `export_dir` setting,
/// or `~/Pictures/MetalVJ` when it is empty.
pub fn export_dir() -> std::path::PathBuf {
    load_at(path())
        .get("export_dir")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(std::path::PathBuf::from)
        .unwrap_or_else(crate::call::frame::default_dir)
}

/// Whether a dropped clip builds its HAP proxy automatically (the editor's
/// first preview then finds the movie already there); `true` when the key is
/// absent, so a hand-edited or older file keeps the automatic build.
pub fn ingest_proxy() -> bool {
    load_at(path()).get("ingest_proxy").and_then(Value::as_bool).unwrap_or(true)
}

/// Whether `v` is a colour rule the built-ins can apply: one of the four
/// groups `call::colour` knows, or an array of node ids (`selection…`).
fn valid_colour_rule(v: &Value) -> bool {
    match v {
        Value::String(s) => matches!(s.as_str(), "all" | "models" | "effects" | "text"),
        Value::Array(ids) => ids.iter().all(|id| id.as_str().is_some_and(|s| !s.is_empty())),
        _ => false,
    }
}

/// Whether `v` is one `[w, h]` span the overlays grid can paint: two numbers,
/// each 1 or 2 (a 2-wide cell is the widest the grid's own clamp allows).
fn valid_span(v: &Value) -> bool {
    match v.as_array().map(Vec::as_slice) {
        Some([w, h]) => match (w.as_f64(), h.as_f64()) {
            (Some(w), Some(h)) => crate::surface::valid_span(w, h),
            _ => false,
        },
        _ => false,
    }
}

/// Whether `v` is one overlay controller the grid can store: an object whose
/// `graph`/`name`, when present, are strings, whose `mode`, when present, is
/// `switch` or `hold`, and whose `targets`, when present, is an array of
/// [`valid_target`]s. An empty object is a valid, cleared controller.
fn valid_controller(v: &Value) -> bool {
    let Some(obj) = v.as_object() else {
        return false;
    };
    for field in ["graph", "name"] {
        if obj.get(field).is_some_and(|s| s.as_str().is_none()) {
            return false;
        }
    }
    if obj.get("mode").is_some_and(|m| !matches!(m.as_str(), Some("switch" | "hold"))) {
        return false;
    }
    match obj.get("targets") {
        None => true,
        Some(t) => t.as_array().is_some_and(|list| list.iter().all(valid_target)),
    }
}

/// Whether `v` is one grid controller target: an object naming a string
/// `node`, with a bool `on`, a string `param` and a number `value`, and a
/// string `fixture`/`group`, when each is present.
fn valid_target(v: &Value) -> bool {
    let Some(obj) = v.as_object() else {
        return false;
    };
    if obj.get("node").and_then(Value::as_str).is_none() {
        return false;
    }
    if obj.get("on").is_some_and(|on| on.as_bool().is_none()) {
        return false;
    }
    for field in ["param", "fixture", "group"] {
        if obj.get(field).is_some_and(|s| s.as_str().is_none()) {
            return false;
        }
    }
    if obj.get("value").is_some_and(|v| v.as_f64().is_none()) {
        return false;
    }
    true
}

/// Whether `v` is one colour tag: an index into `Theme::tag_colours`'s
/// eight-preset ramp (`0..=7`). The ramp itself is the shell's
/// (`classes.rs`); this layer owns only the index's range — the same split
/// `fps_cap` makes against `frame_cap::FPS_CAPS`.
fn valid_tag(v: &Value) -> bool {
    matches!(v.as_u64(), Some(0..=7))
}

/// Validate + write + reply. `shell_mode` obeys `classes::ShellMode`'s own
/// vocabulary (its `parse` names the same three strings), `language` is one
/// of [`LANGUAGES`]. Every key in the body is validated before anything is
/// written, so a body with one bad value writes nothing; a body with neither
/// key is a no-op POST that still answers the current reply.
fn post_at(path: impl AsRef<std::path::Path>, body: &Value) -> Result<Value, String> {
    let path = path.as_ref();
    let mut next = load_at(path);
    let mut changed = false;
    if let Some(mode) = body.get("shell_mode") {
        let s = mode
            .as_str()
            .ok_or_else(|| "shell_mode must be a string".to_string())?;
        // The vocabulary is `crate::shell_mode`'s — top-level so this route
        // validates on every build, window or no window, without reaching
        // into the shell.
        if crate::shell_mode::ShellMode::parse(s).is_none() {
            return Err(format!("unknown shell_mode: {s}"));
        }
        next.insert("shell_mode".into(), json!(s));
        changed = true;
    }
    if let Some(language) = body.get("language") {
        let s = language
            .as_str()
            .ok_or_else(|| "language must be a string".to_string())?;
        if !LANGUAGES.contains(&s) {
            return Err(format!("unknown language: {s} (one of {})", LANGUAGES.join(", ")));
        }
        next.insert("language".into(), json!(s));
        changed = true;
    }
    if let Some(tick) = body.get("tick_level") {
        let s = tick
            .as_str()
            .ok_or_else(|| "tick_level must be a string".to_string())?;
        // One ladder type, one vocabulary: `base` / `fine` today, and a future
        // level is a new arm here with no change to this route.
        if metal_schema::TickRate::parse(s).is_none() {
            return Err(format!(
                "unknown tick_level: {s} (one of {})",
                metal_schema::TickRate::ALL
                    .iter()
                    .map(|r| r.wire())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        next.insert("tick_level".into(), json!(s));
        changed = true;
    }
    if let Some(page) = body.get("grid_page") {
        let n = page
            .as_i64()
            .ok_or_else(|| "grid_page must be an integer".to_string())?;
        next.insert("grid_page".into(), json!(n));
        changed = true;
    }
    if let Some(spans) = body.get("grid_spans") {
        // The whole map, replaced: the panel posts every entry it holds, so
        // a key left out is a key the operator reset to 1×1.
        let obj = spans
            .as_object()
            .ok_or_else(|| "grid_spans must be an object".to_string())?;
        if let Some((key, bad)) = obj.iter().find(|(_, v)| !valid_span(v)) {
            return Err(format!("grid_spans[{key}] must be [w,h], one of 1×1 2×1 1×2 2×2 1×0.5 1×0.3 0.5×1 0.3×1, got {bad}"));
        }
        next.insert("grid_spans".into(), spans.clone());
        changed = true;
    }
    if let Some(surface) = body.get("grid_surface") {
        // The whole layout, replaced, the same contract as `grid_spans`.
        crate::surface::Layout::check(surface).map_err(|e| format!("grid_surface: {e}"))?;
        next.insert(
            "grid_surface".into(),
            crate::surface::Layout::from_value(surface).to_value(),
        );
        changed = true;
    }
    if let Some(ctrls) = body.get("grid_controllers") {
        // The whole map, replaced: the panel posts every controller it holds,
        // so a key left out is a controller the operator cleared — `grid_spans`'s
        // own contract. An explicitly empty object clears every assignment.
        let obj = ctrls
            .as_object()
            .ok_or_else(|| "grid_controllers must be an object".to_string())?;
        if let Some((key, bad)) = obj.iter().find(|(_, v)| !valid_controller(v)) {
            return Err(format!("grid_controllers[{key}] must be a controller object, got {bad}"));
        }
        next.insert("grid_controllers".into(), ctrls.clone());
        changed = true;
    }
    if let Some(view) = body.get("shell_view") {
        let s = view
            .as_str()
            .ok_or_else(|| "shell_view must be a string".to_string())?;
        if !SHELL_VIEWS.contains(&s) {
            return Err(format!("unknown shell_view: {s} (one of {})", SHELL_VIEWS.join(", ")));
        }
        next.insert("shell_view".into(), json!(s));
        changed = true;
    }
    if let Some(rule) = body.get("colour_rule") {
        if !valid_colour_rule(rule) {
            return Err(format!("colour_rule must be all|models|effects|text or an array of node ids, got {rule}"));
        }
        next.insert("colour_rule".into(), rule.clone());
        changed = true;
    }
    if let Some(dir) = body.get("export_dir") {
        let s = dir
            .as_str()
            .ok_or_else(|| "export_dir must be a string".to_string())?;
        next.insert("export_dir".into(), json!(s));
        changed = true;
    }
    if let Some(cap) = body.get("fps_cap") {
        let n = cap
            .as_u64()
            .map(|n| n as u32)
            .filter(|n| FPS_CAPS.contains(n))
            .ok_or_else(|| format!("fps_cap must be one of {FPS_CAPS:?}, got {cap}"))?;
        next.insert("fps_cap".into(), json!(n));
        crate::frame_cap::set(n);
        changed = true;
    }
    if let Some(v) = body.get("ui_scale") {
        let n = v
            .as_u64()
            .map(|n| n as u32)
            .filter(|n| (crate::ui_scale::MIN..=crate::ui_scale::MAX).contains(n))
            .ok_or_else(|| format!("ui_scale must be 75..=200, got {v}"))?;
        next.insert("ui_scale".into(), json!(n));
        crate::ui_scale::set(n);
        changed = true;
    }
    if let Some(tags) = body.get("file_tags") {
        // The whole map, replaced: the panel merges (reads the map, changes
        // the one path, posts it back), so a path left out is a path the
        // operator cleared — `grid_spans`'s own contract.
        let obj = tags
            .as_object()
            .ok_or_else(|| "file_tags must be an object".to_string())?;
        if let Some((path, bad)) = obj.iter().find(|(_, v)| !valid_tag(v)) {
            return Err(format!(
                "file_tags[{path}] must be a colour index 0..=7, got {bad}"
            ));
        }
        next.insert("file_tags".into(), tags.clone());
        changed = true;
    }
    if let Some(on) = body.get("ingest_proxy") {
        let b = on
            .as_bool()
            .ok_or_else(|| "ingest_proxy must be a bool".to_string())?;
        next.insert("ingest_proxy".into(), json!(b));
        changed = true;
    }
    if let Some(name) = body.get("theme") {
        let name = name.as_str().ok_or_else(|| "theme must be a string".to_string())?;
        next.insert("theme".into(), json!(name));
        changed = true;
    }
    if changed {
        let text = Value::Object(next).to_string();
        if let Some(dir) = path.parent() {
            metal_vfs::create_dir_all(dir)
                .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
        }
        crate::atomic::write(path, text).map_err(|e| format!("cannot write settings.json: {e}"))?;
    }
    reply_at(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fps_cap_defaults_to_sixty_and_refuses_off_list_values() {
        let p = tmp_named("fps-cap");
        assert_eq!(reply_at(&p).unwrap()["fps_cap"], json!(60));
        assert!(post_at(&p, &json!({ "fps_cap": 77 })).is_err());
        assert_eq!(post_at(&p, &json!({ "fps_cap": 300 })).unwrap()["fps_cap"], json!(300));
        let _ = post_at(&p, &json!({ "fps_cap": 60 }));
        let _ = metal_vfs::remove_file(&p);
    }

    #[test]
    fn the_ui_scale_persists_and_refuses_out_of_range_values() {
        let p = tmp_named("ui-scale");
        assert_eq!(reply_at(&p).unwrap()["ui_scale"], json!(100));
        assert!(post_at(&p, &json!({ "ui_scale": 50 })).is_err());
        assert!(post_at(&p, &json!({ "ui_scale": 250 })).is_err());
        assert_eq!(post_at(&p, &json!({ "ui_scale": 150 })).unwrap()["ui_scale"], json!(150));
        assert_eq!(reply_at(&p).unwrap()["ui_scale"], json!(150));
        crate::ui_scale::set(100);
        let _ = metal_vfs::remove_file(&p);
    }

    fn tmp() -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("fw-settings-{}.json", std::process::id()));
        let _ = metal_vfs::remove_file(&p);
        p
    }

    /// The layer's own parity promise: a fresh file answers the default,
    /// a saved mode round-trips, and the same reply shape comes back from
    /// both the write and the read — the panel never needs a follow-up GET.
    #[test]
    fn settings_round_trip_through_the_layer() {
        let p = tmp();
        let reply = reply_at(&p).expect("fresh install answers");
        assert_eq!(reply["shell_mode"], json!("auto"));

        let saved = post_at(&p, &json!({ "shell_mode": "mobile" })).expect("save");
        assert_eq!(saved["shell_mode"], json!("mobile"));
        assert_eq!(reply_at(&p).expect("re-read")["shell_mode"], json!("mobile"));
    }

    #[test]
    fn the_language_defaults_to_english_and_round_trips() {
        let p = tmp_named("language");
        assert_eq!(reply_at(&p).expect("fresh install answers")["language"], json!("en"));
        let saved = post_at(&p, &json!({ "language": "es" })).expect("save");
        assert_eq!(saved["language"], json!("es"));
        assert_eq!(saved["shell_mode"], json!("auto"), "the other key keeps its default");
        assert_eq!(reply_at(&p).expect("re-read")["language"], json!("es"));
        let _ = metal_vfs::remove_file(&p);
    }

    /// The tick level is the one machine switch: it defaults to base and moves
    /// to fine (and back) through the same route the shell mode uses. The
    /// vocabulary is `metal_schema::TickRate`'s, so an off-ladder value is refused
    /// rather than stored.
    #[test]
    fn the_tick_level_defaults_to_base_and_round_trips() {
        let p = tmp_named("tick-level");
        assert_eq!(reply_at(&p).expect("fresh install answers")["tick_level"], json!("base"));
        let saved = post_at(&p, &json!({ "tick_level": "fine" })).expect("save");
        assert_eq!(saved["tick_level"], json!("fine"));
        assert_eq!(saved["language"], json!("en"), "the other keys keep their defaults");
        assert_eq!(reply_at(&p).expect("re-read")["tick_level"], json!("fine"));
        // Back to base, losslessly.
        let back = post_at(&p, &json!({ "tick_level": "base" })).expect("save");
        assert_eq!(back["tick_level"], json!("base"));
        let _ = metal_vfs::remove_file(&p);
    }

    #[test]
    fn an_unknown_tick_level_is_refused_rather_than_stored() {
        let p = tmp_named("bad-tick");
        assert!(post_at(&p, &json!({ "tick_level": "30hz" })).is_err());
        assert!(
            metal_vfs::read_to_string(&p).unwrap_or_default().is_empty(),
            "a refused value must not have been written"
        );
    }

    #[test]
    fn a_body_with_one_bad_value_writes_nothing() {
        let p = tmp_named("bad-language");
        assert!(post_at(&p, &json!({ "shell_mode": "mobile", "language": "fr" })).is_err());
        assert!(
            metal_vfs::read_to_string(&p).unwrap_or_default().is_empty(),
            "neither key may land when one is refused"
        );
    }

    /// The route's list and the shell's `Lang` must name the same codes, or
    /// the selector could offer a language the route refuses.
    #[cfg(any(feature = "gpu", feature = "glow"))]
    #[test]
    fn the_route_accepts_exactly_the_shells_languages() {
        let codes: Vec<&str> = crate::shell::i18n::Lang::ALL.iter().map(|l| l.code()).collect();
        assert_eq!(codes, LANGUAGES);
    }

    fn tmp_named(name: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("fw-settings-{name}-{}.json", std::process::id()));
        let _ = metal_vfs::remove_file(&p);
        p
    }

    /// The page steps are blind (no page count here) and the panel clamps:
    /// a step past either end is stored unclamped, a non-integer is refused.
    #[test]
    fn grid_page_steps_are_stored_unclamped_and_a_bad_page_is_refused() {
        let p = tmp_named("grid-page");
        assert_eq!(reply_at(&p).expect("fresh")["grid_page"], json!(0));
        assert_eq!(step_at(&p, 1).expect("next"), 1);
        assert_eq!(step_at(&p, 1).expect("next"), 2);
        assert_eq!(step_at(&p, -1).expect("prev"), 1);
        assert_eq!(step_at(&p, -5).expect("prev past zero"), -4);
        assert_eq!(reply_at(&p).expect("re-read")["grid_page"], json!(-4));
        assert!(post_at(&p, &json!({ "grid_page": "two" })).is_err());
        assert_eq!(reply_at(&p).expect("still")["grid_page"], json!(-4));
    }

    /// The control surface: a fresh install is 8 × 4 with nothing bound, a
    /// whole layout round-trips, and bad dims or an unknown control kind are
    /// refused with nothing written.
    #[test]
    fn grid_surface_round_trips_and_refuses_bad_dims() {
        let p = tmp_named("grid-surface");
        assert_eq!(reply_at(&p).expect("fresh")["grid_surface"]["cols"], json!(8));
        let saved = post_at(
            &p,
            &json!({ "grid_surface": { "cols": 4, "rows": 2, "roles": { "rec-1": { "kind": "rec" } },
                "binds": [{ "input": "note:60", "page": 1, "cell": "rec-1" }] } }),
        )
        .expect("save");
        assert_eq!(saved["grid_surface"]["cols"], json!(4));
        assert_eq!(saved["grid_surface"]["roles"]["rec-1"]["kind"], json!("rec"));
        assert_eq!(saved["grid_surface"]["binds"][0]["page"], json!(1));
        assert!(post_at(&p, &json!({ "grid_surface": { "cols": 0 } })).is_err());
        assert!(post_at(&p, &json!({ "grid_surface": { "roles": { "x": { "kind": "nope" } } } })).is_err());
        assert_eq!(reply_at(&p).expect("re-read")["grid_surface"]["rows"], json!(2));
    }

    /// The overlays grid's cell sizes: a fresh install has none, a full map
    /// round-trips through the route, and a size off `surface::SPANS` (or a
    /// shape that is not `[w,h]`) is refused with nothing written.
    #[test]
    fn grid_spans_round_trip_and_refuse_bad_spans() {
        let p = tmp_named("grid-spans");
        assert_eq!(reply_at(&p).expect("fresh install answers")["grid_spans"], json!({}));
        let saved = post_at(
            &p,
            &json!({ "grid_spans": { "mv:drop": [2, 1], "sw:mood-1": [1, 2] } }),
        )
        .expect("save");
        assert_eq!(saved["grid_spans"]["mv:drop"], json!([2, 1]));
        assert_eq!(saved["grid_spans"]["sw:mood-1"], json!([1, 2]));
        assert_eq!(saved["shell_mode"], json!("auto"), "the other keys keep their defaults");
        // A POST carries the whole map, so a key it leaves out is reset.
        let thin = post_at(&p, &json!({ "grid_spans": { "mv:a": [1, 0.3], "mv:b": [0.5, 1] } })).expect("fractions");
        assert_eq!(thin["grid_spans"]["mv:a"], json!([1, 0.3]));
        let reset = post_at(&p, &json!({ "grid_spans": { "sw:mood-1": [1, 2] } })).expect("save");
        assert!(reset["grid_spans"].get("mv:drop").is_none());

        for bad in [
            json!({ "grid_spans": { "mv:drop": [3, 1] } }),
            json!({ "grid_spans": { "mv:drop": [0.3, 0.3] } }),
            json!({ "grid_spans": { "mv:drop": [0, 1] } }),
            json!({ "grid_spans": { "mv:drop": [1] } }),
            json!({ "grid_spans": { "mv:drop": "2x2" } }),
            json!({ "grid_spans": [2, 2] }),
        ] {
            assert!(post_at(&p, &bad).is_err(), "must refuse {bad}");
        }
        assert_eq!(
            reply_at(&p).expect("re-read")["grid_spans"]["sw:mood-1"],
            json!([1, 2]),
            "a refused body must leave the stored map alone"
        );
        let _ = std::fs::remove_file(&p);
    }

    /// The overlays grid's controller assignments: a fresh install assigns
    /// none, the example shape round-trips through the route (the whole map,
    /// replaced), and a wrong shape — a non-object map, a target without a
    /// `node`, or a mistyped `value` — is refused with nothing written.
    #[test]
    fn grid_controllers_round_trip_and_refuse_bad_shapes() {
        let p = tmp_named("grid-controllers");
        assert_eq!(reply_at(&p).expect("fresh install answers")["grid_controllers"], json!({}));
        let example = json!({
            "ctrl:1": {
                "graph": "g1",
                "name": "button 1",
                "mode": "hold",
                "targets": [
                    { "node": "gen-1", "on": true },
                    { "node": "gen-2", "param": "scale", "value": 0.5 },
                    { "node": "venue-1", "fixture": "wash-1", "on": true },
                    { "node": "venue-1", "group": "front-wash", "on": true },
                ],
            },
        });
        let saved = post_at(&p, &json!({ "grid_controllers": example })).expect("save");
        assert_eq!(saved["grid_controllers"], example);
        assert_eq!(saved["shell_mode"], json!("auto"), "the other keys keep their defaults");
        assert_eq!(reply_at(&p).expect("re-read")["grid_controllers"], example);

        // A POST carries the whole map, so a key it leaves out is cleared; an
        // empty object clears every assignment.
        let cleared = post_at(&p, &json!({ "grid_controllers": {} })).expect("clear");
        assert_eq!(cleared["grid_controllers"], json!({}));

        for bad in [
            json!({ "grid_controllers": ["ctrl:1"] }),
            json!({ "grid_controllers": { "ctrl:1": { "targets": [{ "on": true }] } } }),
            json!({ "grid_controllers": { "ctrl:1": { "targets": [{ "node": "gen-2", "value": "half" }] } } }),
            json!({ "grid_controllers": { "ctrl:1": { "targets": [{ "node": "gen-2", "fixture": 7 }] } } }),
            json!({ "grid_controllers": { "ctrl:1": { "targets": [{ "node": "gen-2", "group": 7 }] } } }),
            json!({ "grid_controllers": { "ctrl:1": { "graph": 7 } } }),
            json!({ "grid_controllers": { "ctrl:1": { "mode": "latch" } } }),
        ] {
            assert!(post_at(&p, &bad).is_err(), "must refuse {bad}");
        }
        assert_eq!(
            reply_at(&p).expect("re-read")["grid_controllers"],
            json!({}),
            "a refused body must leave the stored map alone"
        );
        let _ = std::fs::remove_file(&p);
    }

    /// The view defaults to edit, round-trips show and edit, and an
    /// off-vocabulary value is refused with the stored view left alone.
    #[test]
    fn the_shell_view_defaults_to_edit_and_round_trips() {
        let p = tmp_named("shell-view");
        assert_eq!(reply_at(&p).expect("fresh install answers")["shell_view"], json!("edit"));
        let saved = post_at(&p, &json!({ "shell_view": "show" })).expect("save");
        assert_eq!(saved["shell_view"], json!("show"));
        assert_eq!(saved["shell_mode"], json!("auto"), "the other keys keep their defaults");
        assert_eq!(reply_at(&p).expect("re-read")["shell_view"], json!("show"));
        assert!(post_at(&p, &json!({ "shell_view": "kiosk" })).is_err());
        assert_eq!(
            reply_at(&p).expect("re-read")["shell_view"],
            json!("show"),
            "a refused view must leave the stored one alone"
        );
        let _ = std::fs::remove_file(&p);
    }

    /// The route's view list and `shell::ShellView` must name the same codes.
    #[cfg(any(feature = "gpu", feature = "glow"))]
    #[test]
    fn the_route_accepts_exactly_the_shells_views() {
        assert_eq!(crate::shell::ShellView::CODES, SHELL_VIEWS);
    }

    #[test]
    fn colour_rule_defaults_to_all_round_trips_and_refuses_bad_rules() {
        let p = tmp_named("colour-rule");
        assert_eq!(reply_at(&p).expect("fresh install answers")["colour_rule"], json!("all"));
        let saved = post_at(&p, &json!({ "colour_rule": "effects" })).expect("save");
        assert_eq!(saved["colour_rule"], json!("effects"));
        let picked = post_at(&p, &json!({ "colour_rule": ["gen-1", "fx-2"] })).expect("save ids");
        assert_eq!(picked["colour_rule"], json!(["gen-1", "fx-2"]));
        for bad in [
            json!({ "colour_rule": "everything" }),
            json!({ "colour_rule": [1, 2] }),
            json!({ "colour_rule": 7 }),
        ] {
            assert!(post_at(&p, &bad).is_err(), "must refuse {bad}");
        }
        assert_eq!(reply_at(&p).expect("re-read")["colour_rule"], json!(["gen-1", "fx-2"]));
        let _ = std::fs::remove_file(&p);
    }

    /// The biblioteca's colour tags: a fresh install has none, a map of path
    /// → index round-trips through the route (the whole map, replaced), and
    /// an index outside 0..=7 (or a shape that is not an object) is refused
    /// with nothing written.
    #[test]
    fn file_tags_default_empty_round_trip_and_refuse_bad_indices() {
        let p = tmp_named("file-tags");
        assert_eq!(reply_at(&p).expect("fresh install answers")["file_tags"], json!({}));
        let saved = post_at(
            &p,
            &json!({ "file_tags": { "/music/a.mp3": 0, "/music/clips": 7 } }),
        )
        .expect("save");
        assert_eq!(saved["file_tags"]["/music/a.mp3"], json!(0));
        assert_eq!(saved["file_tags"]["/music/clips"], json!(7));
        assert_eq!(saved["shell_mode"], json!("auto"), "the other keys keep their defaults");
        // The panel clears a tag by leaving its path out of the posted map.
        let cleared = post_at(&p, &json!({ "file_tags": { "/music/clips": 7 } })).expect("save");
        assert!(cleared["file_tags"].get("/music/a.mp3").is_none());
        for bad in [
            json!({ "file_tags": { "/music/a.mp3": 8 } }),
            json!({ "file_tags": { "/music/a.mp3": -1 } }),
            json!({ "file_tags": { "/music/a.mp3": "red" } }),
            json!({ "file_tags": ["red"] }),
        ] {
            assert!(post_at(&p, &bad).is_err(), "must refuse {bad}");
        }
        assert_eq!(
            reply_at(&p).expect("re-read")["file_tags"]["/music/clips"],
            json!(7),
            "a refused body must leave the stored map alone"
        );
        let _ = std::fs::remove_file(&p);
    }

    /// The ingest proxy toggle: it defaults to on (a dropped clip builds its
    /// proxy automatically), round-trips a bool through the route, and a
    /// non-bool is refused with nothing written.
    #[test]
    fn the_ingest_proxy_toggle_defaults_on_and_round_trips_a_bool() {
        let p = tmp_named("ingest-proxy");
        assert_eq!(reply_at(&p).expect("fresh install answers")["ingest_proxy"], json!(true));
        let off = post_at(&p, &json!({ "ingest_proxy": false })).expect("save");
        assert_eq!(off["ingest_proxy"], json!(false));
        assert_eq!(reply_at(&p).expect("re-read")["ingest_proxy"], json!(false));
        let back = post_at(&p, &json!({ "ingest_proxy": true })).expect("save");
        assert_eq!(back["ingest_proxy"], json!(true));
        assert!(post_at(&p, &json!({ "ingest_proxy": "yes" })).is_err());
        assert_eq!(
            reply_at(&p).expect("re-read")["ingest_proxy"],
            json!(true),
            "a refused value must leave the stored one alone"
        );
        let _ = metal_vfs::remove_file(&p);
    }

    #[test]
    fn an_unknown_shell_mode_is_refused_rather_than_stored() {
        let p = tmp();
        assert!(post_at(&p, &json!({ "shell_mode": "kiosk" })).is_err());
        let on_disk = metal_vfs::read_to_string(&p).unwrap_or_default();
        assert!(
            on_disk.is_empty(),
            "a refused value must not have been written"
        );
    }
}
