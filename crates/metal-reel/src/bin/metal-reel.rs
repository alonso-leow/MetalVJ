//! Indexing a folder from a terminal, and looking at what came out.
//!
//! The app does the same thing from a button, and this exists for the two cases
//! a button is bad at: indexing three hours of rushes without a browser tab
//! open on it, and answering *"what does this library actually look like"*
//! without a GPU. `metal-ingest` is the same shape for the dance half.

use std::path::PathBuf;

use metal_reel::index::{self, Library, Options, Step};
use metal_reel::read::NAMES;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let code = match refs.as_slice() {
        ["index", source, name, rest @ ..] => build(source, name, rest),
        ["info", name, rest @ ..] => info(name, rest),
        ["list", rest @ ..] => list(rest),
        _ => {
            eprintln!(
                "metal-reel — a folder of footage, indexed\n\
                 \n\
                   metal-reel index <folder> <name> [--data DIR] [--rate N] [--width N]\n\
                   metal-reel info  <name>          [--data DIR]\n\
                   metal-reel list                  [--data DIR]\n\
                 \n\
                 The folder is only ever read. The index and the frame ladder go\n\
                 to <data>/reels/<name>/."
            );
            2
        }
    };
    std::process::exit(code);
}

fn flag(rest: &[&str], name: &str) -> Option<String> {
    rest.iter().position(|a| *a == name).and_then(|i| rest.get(i + 1)).map(|s| s.to_string())
}

fn data_dir(rest: &[&str]) -> PathBuf {
    flag(rest, "--data").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("data"))
}

fn build(source: &str, name: &str, rest: &[&str]) -> i32 {
    let mut opts = Options::default();
    if let Some(r) = flag(rest, "--rate").and_then(|v| v.parse().ok()) {
        opts.rate = r;
    }
    if let Some(w) = flag(rest, "--width").and_then(|v| v.parse().ok()) {
        opts.width = w;
    }
    let data = data_dir(rest);
    println!(
        "indexing {source} as {name} — {} rungs a second at {} px, into {}",
        opts.rate,
        opts.width,
        Library::dir(&data, name).display()
    );

    let mut last = String::new();
    let result = index::build(
        PathBuf::from(source).as_path(),
        &data,
        name,
        &opts,
        &mut |step| match step {
            Step::Probing { file, clip, of } => println!("[{}/{of}] {file}", clip + 1),
            Step::Extracting { .. } => print!("      extracting… "),
            Step::Reading { done, total, .. } => {
                // One line per clip rather than one per rung: a folder of two
                // hundred clips is otherwise ten thousand lines of scrollback.
                let line = format!("reading {done}/{total}");
                if line != last {
                    print!("\r      {line}          ");
                    let _ = std::io::Write::flush(&mut std::io::stdout());
                    last = line;
                }
            }
            Step::Skipped { file, why } => println!("      skipped {file}: {why}"),
            Step::Encoding { file, .. } => println!("      HAP {file}"),
            Step::Done { clips, rungs, bytes } => println!(
                "\r{clips} clips, {rungs} rungs, {:.1} MB on disk          ",
                bytes as f64 / 1e6
            ),
        },
    );

    match result {
        Ok(lib) => {
            for c in &lib.clips {
                println!("  {:<28} {:>6.1}s  {} passages", c.id, c.seconds, c.passages.len());
            }
            0
        }
        Err(e) => {
            eprintln!("{e}");
            1
        }
    }
}

fn info(name: &str, rest: &[&str]) -> i32 {
    let data = data_dir(rest);
    let lib = match Library::load(&data, name) {
        Ok(lib) => lib,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    println!(
        "{} — {} clips, {:.1}s, {} rungs at {} Hz / {} px, from {}",
        lib.name,
        lib.clips.len(),
        lib.seconds(),
        lib.rungs(),
        lib.rate,
        lib.width,
        lib.source
    );
    for (ci, c) in lib.clips.iter().enumerate() {
        println!("\n  [{ci}] {} ({})  {:.1}s", c.id, c.file, c.seconds);
        for (pi, p) in c.passages.iter().enumerate() {
            let at = p.from as f32 / lib.rate;
            let span = (p.to - p.from) as f32 / lib.rate;
            println!("      passage {pi}  {at:>6.1}s +{span:.1}s");
            // The descriptor, named. This is the only view anywhere of what the
            // Curator is actually matching on.
            let mut line = String::from("        ");
            for (i, v) in p.d.iter().enumerate() {
                line.push_str(&format!("{}={:.2}  ", NAMES[i], v));
            }
            println!("{line}");
        }
    }
    0
}

fn list(rest: &[&str]) -> i32 {
    let data = data_dir(rest);
    let names = Library::list(&data);
    if names.is_empty() {
        println!("no libraries under {}", data.join(index::DIR).display());
        return 0;
    }
    for name in names {
        match Library::load(&data, &name) {
            Ok(lib) => println!(
                "{:<20} {:>4} clips  {:>7.1}s  {}",
                lib.name,
                lib.clips.len(),
                lib.seconds(),
                lib.source
            ),
            Err(e) => println!("{name:<20} unreadable: {e}"),
        }
    }
    0
}
