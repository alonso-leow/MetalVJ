//! One frame of one model, to a JPEG.
//! `cargo run -p metal-render --example shot -- <id> <out.jpg> [time] [w] [h] [ghost] [kaleido] [steps]`
//! `steps` walks the clock up to `time` in that many frames first, which is
//! the only way to see a trail: ghosting is a function of the frames before it.
//!
//! For a **transmission** — a model that draws over footage — add
//! `--reel <data-dir> <library> [clip] [passage]`. Without it the model draws
//! its own slate, which is a legitimate thing to want to look at: it is what a
//! fresh install shows.
//!
//! `--set <float>=<value>` (repeatable) moves one float off its default, **by
//! name**. It is here because the one thing no gate in this project can check is
//! whether a frame is any good — `CORRECTIONS.md` says so three times — and
//! looking at one meant editing a default and rebuilding until now. Names rather
//! than positions, so a command line in a commit message still means the same
//! thing after somebody inserts a float.
use metal_render::headless::{encode_jpeg, Renderer};
use metal_render::preset::Post;
use metal_schema::Block;

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    // The positional arguments stop at the first flag, so the spellings do not
    // have to know about each other.
    let sets: Vec<&str> = a
        .iter()
        .enumerate()
        .filter(|(i, s)| *s == "--set" && *i + 1 < a.len())
        .map(|(i, _)| a[i + 1].as_str())
        .collect();
    let split = a
        .iter()
        .position(|s| s.starts_with("--"))
        .unwrap_or(a.len());
    let pos = &a[..split];
    let reel_at = a.iter().position(|s| s == "--reel").unwrap_or(a.len());
    // Up to the next flag, so `--reel … --set …` cannot have a `--set` parsed
    // as a passage number — which fails by rendering the wrong shot silently.
    let reel_end = a
        .iter()
        .skip(reel_at + 1)
        .position(|s| s.starts_with("--"))
        .map(|k| reel_at + 1 + k)
        .unwrap_or(a.len());
    let reel = &a[reel_at..reel_end];
    let num = |i: usize, d: f32| pos.get(i).and_then(|s| s.parse().ok()).unwrap_or(d);

    let mut r = Renderer::new();
    let ids = r.ids();
    let want = pos.first().cloned().unwrap_or_else(|| "geoterrain".into());
    let i = ids.iter().position(|id| *id == want).expect("no such model");
    let out = pos.get(1).cloned().unwrap_or_else(|| "/tmp/shot.jpg".into());
    let (t, w, h) = (num(2, 3.0), num(3, 640.0) as u32, num(4, 360.0) as u32);
    let post = Post { ghost: num(5, 0.0), kaleido: num(6, 0.0), ..Post::default() };
    let steps = num(7, 1.0) as u32;

    if let (Some(data), Some(name)) = (reel.get(1), reel.get(2)) {
        let lib = metal_reel::Library::load(std::path::Path::new(data), name)
            .expect("no such library");
        let clip: usize = reel.get(3).and_then(|s| s.parse().ok()).unwrap_or(0);
        let passage: usize = reel.get(4).and_then(|s| s.parse().ok()).unwrap_or(0);
        let c = lib.clips.get(clip).expect("no such clip");
        let p = c.passages.get(passage).copied().expect("no such passage");
        let cue = metal_reel::Cue {
            clip,
            clip_id: c.id.clone(),
            passage,
            from: p.from as f32 / lib.rate,
            span: (p.to - p.from) as f32 / lib.rate,
            // Cued at zero, so `time` reads as seconds into the passage and a
            // shot is reproducible from its own command line.
            since: 0.0,
            confidence: 1.0,
            because: None,
            play: metal_reel::curate::Play::default(),
            // Named on the command line rather than recalled from a lane.
            authored: None,
        };
        eprintln!(
            "{}/{} passage {passage}: {:.1}s +{:.1}s",
            lib.name, c.id, cue.from, cue.span
        );
        let r = r.reel(i).expect("that model does not take footage");
        r.attach(Some(std::sync::Arc::new(lib)));
        r.cue(Some(cue));
    }

    let mut row = r.schemas()[i].defaults();
    // Write one named float, clamped to its declared range — so a shot cannot
    // show a state the app could never reach, which would make it evidence of
    // nothing.
    let mut put = |row: &mut Vec<f32>, name: &str, value: f32| {
        let mut at = 0usize;
        let mut found = false;
        for block in r.schemas()[i].blocks {
            if let Block::Floats(specs) = block {
                for (k, spec) in specs.iter().enumerate() {
                    if spec.name == name {
                        row[at + k] = value.clamp(spec.min, spec.max);
                        found = true;
                    }
                }
            }
            at += block.width();
        }
        assert!(found, "that model has no float called {name:?}");
    };
    // The two positional post slots are the same two floats as everything else
    // now that the effects blocks are part of every schema, so they are written
    // by name like any other. The positions stay so old command lines in commit
    // messages still mean what they said.
    put(&mut row, "ghost", post.ghost);
    put(&mut row, "kaleido", post.kaleido);
    for set in &sets {
        let (name, value) = set.split_once('=').expect("--set wants name=value");
        put(&mut row, name, value.parse().expect("--set wants a number"));
    }
    for k in 1..steps {
        r.solo(i, &row, t - (steps - k) as f32 / 30.0, w, h, false);
    }
    let (px, pw, ph) = r.solo(i, &row, t, w, h, false);
    std::fs::write(&out, encode_jpeg(&px, pw, ph)).expect("write");
    eprintln!("{want} -> {out}");
}
