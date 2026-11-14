//! Imported desk scenes (`data/stage-scenes/`, local only): the scenes
//! gallery lists them, the properties page edits one, and the lanes import one
//! as a clip of hitbox matrix floats ([`metal_stage::scene::to_clip`]).
//!
//! Layout: `scenes/<slug>.json` (one [`Scene`] each), `placements.json`
//! (`by_id: {desk id: "l2.back.l" | "l2.pro.x.beta"}`: a place, or a place and its variant), `fixtures.json` (`[{id, type}]`).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

use metal_stage::scene::{self, Clip, Scene};
use metal_stage::{Kind, Spot};
use serde::Serialize;

pub const DIR: &str = "data/stage-scenes";

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct Summary {
    pub slug: String,
    pub name: String,
    pub folder: String,
    pub steps: usize,
    pub length_ms: u32,
    pub tags: Vec<&'static str>,
    pub fixtures: usize,
}

fn scenes_dir(root: &Path) -> PathBuf {
    root.join("scenes")
}

fn valid_slug(slug: &str) -> bool {
    !slug.is_empty() && slug.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn summary(slug: String, s: &Scene) -> Summary {
    let mut ids = std::collections::BTreeSet::new();
    for st in &s.steps {
        ids.extend(st.fixtures.iter().map(|f| f.id));
    }
    Summary {
        slug,
        name: s.name.clone(),
        folder: s.folder.clone(),
        steps: s.steps.len(),
        length_ms: s.length_ms(),
        tags: scene::tags(s),
        fixtures: ids.len(),
    }
}

type Cache = Option<(PathBuf, SystemTime, Vec<Summary>)>;
static CACHE: Mutex<Cache> = Mutex::new(None);

/// Every scene, sorted by folder then name. Cached until the folder changes.
pub fn list(root: &Path) -> Vec<Summary> {
    let dir = scenes_dir(root);
    let stamp = std::fs::metadata(&dir).and_then(|m| m.modified()).ok();
    if let (Some(stamp), Ok(c)) = (stamp, CACHE.lock()) {
        if let Some((p, t, rows)) = c.as_ref() {
            if *p == dir && *t == stamp {
                return rows.clone();
            }
        }
    }
    let mut rows: Vec<Summary> = std::fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            let slug = p.file_stem()?.to_str()?.to_string();
            if p.extension()? != "json" {
                return None;
            }
            let s: Scene = serde_json::from_slice(&std::fs::read(&p).ok()?).ok()?;
            Some(summary(slug, &s))
        })
        .collect();
    rows.sort_by(|a, b| (&a.folder, &a.name, &a.slug).cmp(&(&b.folder, &b.name, &b.slug)));
    if let (Some(stamp), Ok(mut c)) = (stamp, CACHE.lock()) {
        *c = Some((dir, stamp, rows.clone()));
    }
    rows
}

pub fn load(root: &Path, slug: &str) -> Result<Scene, String> {
    if !valid_slug(slug) {
        return Err(format!("bad scene slug {slug:?}"));
    }
    let p = scenes_dir(root).join(format!("{slug}.json"));
    let bytes = std::fs::read(&p).map_err(|e| format!("{}: {e}", p.display()))?;
    let mut s: Scene = serde_json::from_slice(&bytes).map_err(|e| format!("{}: {e}", p.display()))?;
    s.slug = slug.to_string();
    Ok(s)
}

/// Writes `scene` under its slug (an edit, or a copy under a new slug).
pub fn save(root: &Path, scene: &Scene) -> Result<Summary, String> {
    if !valid_slug(&scene.slug) {
        return Err(format!("bad scene slug {:?}", scene.slug));
    }
    let dir = scenes_dir(root);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let body = serde_json::to_vec(scene).map_err(|e| e.to_string())?;
    std::fs::write(dir.join(format!("{}.json", scene.slug)), body).map_err(|e| e.to_string())?;
    if let Ok(mut c) = CACHE.lock() {
        *c = None;
    }
    Ok(summary(scene.slug.clone(), scene))
}

/// Desk id → hitbox point and kind, from the import's side files.
pub fn maps(root: &Path) -> (HashMap<u64, Spot>, HashMap<u64, Kind>) {
    let read = |name: &str| -> serde_json::Value {
        std::fs::read(root.join(name)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    };
    let placements = read("placements.json");
    let points = placements["by_id"]
        .as_object()
        .into_iter()
        .flatten()
        .filter_map(|(id, p)| Some((id.parse().ok()?, Spot::parse(p.as_str()?)?)))
        .collect();
    let kinds = read("fixtures.json")
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|f| Some((f["id"].as_u64()?, scene::kind_of_desk_type(f["type"].as_str()?)?)))
        .collect();
    (points, kinds)
}

pub fn clip(root: &Path, slug: &str) -> Result<Clip, String> {
    let s = load(root, slug)?;
    let (p, k) = maps(root);
    Ok(scene::to_clip(&s, &p, &k))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root() -> PathBuf {
        let d = std::env::temp_dir().join(format!("mvj-stage-scenes-{}-{:?}", std::process::id(), std::thread::current().id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("scenes")).unwrap();
        std::fs::write(d.join("placements.json"), r#"{"by_id":{"7":"l2.back.l"}}"#).unwrap();
        std::fs::write(d.join("fixtures.json"), r#"[{"id":7,"type":"beam"}]"#).unwrap();
        std::fs::write(
            d.join("scenes/a-one.json"),
            r#"{"name":"One","folder":"F","format":"scex","steps":[{"length":400,"fixtures":[{"id":7,"model":"m","channels":{"level":{"norm":1.0,"fade":true}}}]}]}"#,
        )
        .unwrap();
        d
    }

    #[test]
    fn lists_loads_saves_and_clips() {
        let r = root();
        let rows = list(&r);
        assert_eq!(rows.len(), 1);
        assert_eq!((rows[0].slug.as_str(), rows[0].length_ms, rows[0].fixtures), ("a-one", 400, 1));
        let mut s = load(&r, "a-one").unwrap();
        s.slug = "a-two".into();
        s.name = "Two".into();
        s.steps[0].length = 800;
        save(&r, &s).unwrap();
        assert_eq!(list(&r).len(), 2);
        assert_eq!(load(&r, "a-two").unwrap().rest["format"], "scex");
        let c = clip(&r, "a-two").unwrap();
        assert_eq!(c.columns, vec!["l2.back.l.moving.level"]);
        assert_eq!(c.length_ms, 800);
        assert!(load(&r, "../x").is_err());
        let _ = std::fs::remove_dir_all(&r);
    }
}
