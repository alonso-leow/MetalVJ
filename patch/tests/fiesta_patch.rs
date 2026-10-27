//! The Fiesta Resort venue's patch, as the Settings > Fixtures table reads it.
//!
//! `GET /venue` reports one row per `[[fixture]]` with the DMX span its profile
//! occupies, and the settings page paints exactly that. Two invariants the
//! page silently depends on: every row resolves a profile (a missing one gives
//! `span = 0`, a row that looks unrouted), and every address is inside
//! `1..=512` because `clamp_address` rewrites anything else on the next edit
//! -- a fixture parked above 512 is on the *next universe*, not slot 600.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use metal_perform::fixture;
use metal_perform::venue;

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn the_venue() -> venue::Venue {
    let text = std::fs::read_to_string(repo().join("patch/venues/fiesta-resort.toml"))
        .expect("fiesta-resort.toml on disk");
    venue::Venue::parse(&text).expect("parses")
}

#[test]
fn every_patch_row_resolves_a_profile_and_a_span() {
    let v = the_venue();
    assert!(!v.fixtures.is_empty());

    for f in &v.fixtures {
        let name = f.profile.clone().unwrap_or_else(|| panic!("{} has no profile", f.id));
        let p = fixture::get(&repo().join("data"), &name)
            .unwrap_or_else(|| panic!("{}: profile {name:?} does not load", f.id));
        // The span the settings page computes: the highest slot the profile
        // writes, plus one.
        let span = p
            .channels
            .iter()
            .flat_map(|c| c.coarse.into_iter().chain(c.fine))
            .max()
            .map(|last| last + 1)
            .unwrap_or(0);
        assert!(span > 0, "{} ({name}) packs no channels: its row would read span 0", f.id);
    }
}

#[test]
fn no_fixture_sits_past_slot_512() {
    let v = the_venue();
    for f in &v.fixtures {
        let a = f.address.unwrap_or(1);
        assert!(
            (1..=512).contains(&a),
            "{} is at address {a}: a universe holds 1..=512, and clamp_address would rewrite this",
            f.id
        );
    }
}

#[test]
fn the_two_smoke_machines_are_the_only_shared_address() {
    // ShowXpress patches Humo&Neblina as `quantity = 2`: one row, two units.
    let v = the_venue();
    let mut seen: BTreeMap<(u16, u16), Vec<&str>> = BTreeMap::new();
    for f in &v.fixtures {
        let key = (f.universe.unwrap_or(0), f.address.unwrap_or(1));
        seen.entry(key).or_default().push(f.id.as_str());
    }
    for (key, ids) in &seen {
        if ids.len() > 1 {
            let mut sorted = ids.clone();
            sorted.sort();
            assert_eq!(
                sorted,
                vec!["humo-neblina-1", "humo-neblina-2"],
                "unexpected shared address at {key:?}"
            );
        }
    }
    assert_eq!(seen.values().filter(|v| v.len() > 1).count(), 1);
}

#[test]
fn the_universe_split_matches_showxpress_numbering() {
    // ShowXpress counts addresses straight through: 513 is universe 1 slot 1.
    let v = the_venue();
    let u1: Vec<_> = v.fixtures.iter().filter(|f| f.universe == Some(1)).collect();
    assert!(!u1.is_empty(), "the rig uses a second universe");
    for f in u1 {
        // slot = showxpress address - 512
        assert!(matches!(f.address, Some(a) if a <= 214), "{}", f.id);
    }
    let u0: Vec<_> = v.fixtures.iter().filter(|f| f.universe.unwrap_or(0) == 0).collect();
    for f in u0 {
        assert!(f.address.unwrap_or(1) <= 502, "{}", f.id);
    }
}

#[test]
fn the_production_camera_shares_the_camera_shot_pose() {
    let v = the_venue();
    let s = v.sensor("production-cam").expect("the webcam is placed");
    let shot = v.shot("production").expect("the camera shot exists");
    assert_eq!(s.at, shot.camera.eye, "sensor stands where the shot says");
    assert_eq!(s.yaw, shot.camera.yaw);
    assert_eq!(s.pitch, shot.camera.pitch);
    assert_eq!(s.fov, shot.camera.fov);
    let src = s.capture.as_deref().expect("the sensor names a capture source");
    assert!(src.starts_with("dshow:") || src.starts_with("v4l2:"), "{src}");
}

