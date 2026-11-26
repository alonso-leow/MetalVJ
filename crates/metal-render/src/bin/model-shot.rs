//! Stills of one model with chosen floats, headless. For checking a float's
//! effect by eye, without authoring an example first.
//!
//!     cargo run -p metal-render --bin model-shot -- --model ripples \
//!         --set layout=3 --set skew=0.7 --at 0,2.5 --out <dir>
//!
//! Floats not named with `--set` keep the schema's own default. `--set` takes
//! the float's name; the first block that declares it wins. Writes
//! `<out>/<model>-<tag>-<t>.jpg` for every `--at` time.
//!
//! `--sweep <prefix>` renders every float whose name starts with the prefix,
//! one at each of `--values` (others at default), at the first `--at` time,
//! as `<out>/<model>-<name>=<value>.jpg`, plus `<model>-base.jpg`.

use metal_render::headless::{encode_jpeg, Renderer};
use metal_schema::Block;

fn main() {
    let mut model = String::new();
    let mut sets: Vec<(String, f32)> = Vec::new();
    let mut at: Vec<f32> = vec![0.0];
    let mut out = String::from(".");
    let mut tag = String::from("shot");
    let mut sweep: Option<String> = None;
    let mut values: Vec<f32> = vec![15.0, -15.0, 50.0, -50.0];
    let (mut w, mut h) = (960u32, 540u32);
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        let mut val = || it.next().unwrap_or_else(|| panic!("{flag} needs a value"));
        match flag.as_str() {
            "--model" => model = val(),
            "--set" => {
                let v = val();
                let (k, x) = v.split_once('=').expect("--set name=value");
                sets.push((k.to_string(), x.parse().expect("--set value must be a number")));
            }
            "--at" => at = val().split(',').map(|s| s.trim().parse().expect("bad --at")).collect(),
            "--out" => out = val(),
            "--tag" => tag = val(),
            "--sweep" => sweep = Some(val()),
            "--values" => values = val().split(',').map(|s| s.trim().parse().expect("bad --values")).collect(),
            "--w" => w = val().parse().expect("bad --w"),
            "--h" => h = val().parse().expect("bad --h"),
            other => panic!("unknown argument {other}"),
        }
    }
    assert!(!model.is_empty(), "--model <id> is required");
    std::fs::create_dir_all(&out).expect("create --out");

    let mut renderer = Renderer::new();
    let ids = renderer.ids();
    let i = ids.iter().position(|id| *id == model).unwrap_or_else(|| panic!("no model {model}"));
    let schema = renderer.schemas()[i];
    let mut floats = schema.defaults();
    for (name, value) in &sets {
        let mut found = false;
        let mut off = 0usize;
        for b in schema.blocks {
            if let Block::Floats(fs) = b {
                if let Some(k) = fs.iter().position(|s| s.name == name) {
                    floats[off + k] = *value;
                    found = true;
                    break;
                }
            }
            off += b.width();
        }
        assert!(found, "{model} has no float named {name}");
    }

    if let Some(prefix) = sweep {
        let t = at[0];
        let names: Vec<(usize, &'static str)> = {
            let mut v = Vec::new();
            let mut off = 0usize;
            for b in schema.blocks {
                if let Block::Floats(fs) = b {
                    for (k, sp) in fs.iter().enumerate() {
                        if sp.name.starts_with(&prefix) {
                            v.push((off + k, sp.name));
                        }
                    }
                }
                off += b.width();
            }
            v
        };
        let base = floats.clone();
        let (rgba, rw, rh) = renderer.solo(i, &base, t, w, h, false);
        std::fs::write(format!("{out}/{model}-base.jpg"), encode_jpeg(&rgba, rw, rh)).expect("write");
        for (idx, name) in names {
            for &v in &values {
                let mut f = base.clone();
                f[idx] = v;
                let (rgba, rw, rh) = renderer.solo(i, &f, t, w, h, false);
                std::fs::write(format!("{out}/{model}-{name}={v}.jpg"), encode_jpeg(&rgba, rw, rh)).expect("write");
            }
            println!("{name}");
        }
        return;
    }

    for t in at {
        let (rgba, rw, rh) = renderer.solo(i, &floats, t, w, h, false);
        let path = format!("{out}/{model}-{tag}-{t:.1}.jpg");
        std::fs::write(&path, encode_jpeg(&rgba, rw, rh)).expect("write still");
        println!("{path}");
    }
}
