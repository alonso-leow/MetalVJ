//! Child processes, and the one place that knows they do not exist on Android.
//!
//! The app shells out to `ffmpeg`, `ffplay`, `ffprobe`, `pactl`, `niri`,
//! `python3`, `node`, `rustc`, `curl`, `tar`, `df`, `mkfifo`. None of them ship
//! with Android, so a spawn there is a bare `ENOENT` that every caller already
//! turns into "no result". This wrapper makes that explicit: on
//! `target_os = "android"` every process-running method returns
//! `io::ErrorKind::Unsupported` with `"<program>: unavailable on this platform"`
//! and never touches `execve`. Everywhere else it is `std::process::Command`,
//! method for method.
//!
//! Use it as a drop-in: `use metal_schema::platform::Command;`.

use std::ffi::OsStr;
use std::io;
use std::process::{Child, ExitStatus, Output, Stdio};

/// True where child processes are unavailable (Android).
pub const NO_PROCESSES: bool = cfg!(target_os = "android");

/// The error a short-circuited spawn returns.
pub fn unavailable(program: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::Unsupported,
        format!("{program}: unavailable on this platform (Android has no child processes)"),
    )
}

/// The refusal for `program` on a platform where `no_processes` holds, else
/// `None`. The one decision every [`Command`] method takes; a function of its
/// inputs so a desktop test can check the Android answer.
pub fn refusal(program: &str, no_processes: bool) -> Option<io::Error> {
    no_processes.then(|| unavailable(program))
}

#[derive(Debug)]
pub struct Command {
    inner: std::process::Command,
    program: String,
}

impl Command {
    pub fn new<S: AsRef<OsStr>>(program: S) -> Self {
        let program = program.as_ref().to_string_lossy().into_owned();
        Self { inner: std::process::Command::new(&program), program }
    }

    /// The underlying `std` command, for the rare caller that needs more.
    pub fn as_std_mut(&mut self) -> &mut std::process::Command {
        &mut self.inner
    }

    pub fn arg<S: AsRef<OsStr>>(&mut self, arg: S) -> &mut Self {
        self.inner.arg(arg);
        self
    }

    pub fn args<I, S>(&mut self, args: I) -> &mut Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.inner.args(args);
        self
    }

    pub fn env<K: AsRef<OsStr>, V: AsRef<OsStr>>(&mut self, key: K, val: V) -> &mut Self {
        self.inner.env(key, val);
        self
    }

    pub fn current_dir<P: AsRef<std::path::Path>>(&mut self, dir: P) -> &mut Self {
        self.inner.current_dir(dir);
        self
    }

    pub fn stdin<T: Into<Stdio>>(&mut self, cfg: T) -> &mut Self {
        self.inner.stdin(cfg);
        self
    }

    pub fn stdout<T: Into<Stdio>>(&mut self, cfg: T) -> &mut Self {
        self.inner.stdout(cfg);
        self
    }

    pub fn stderr<T: Into<Stdio>>(&mut self, cfg: T) -> &mut Self {
        self.inner.stderr(cfg);
        self
    }

    /// Unix only, as in `std`: the child leads its own process group.
    #[cfg(unix)]
    pub fn process_group(&mut self, pgroup: i32) -> &mut Self {
        std::os::unix::process::CommandExt::process_group(&mut self.inner, pgroup);
        self
    }

    pub fn output(&mut self) -> io::Result<Output> {
        if let Some(e) = refusal(&self.program, NO_PROCESSES) {
            return Err(e);
        }
        self.inner.output()
    }

    pub fn status(&mut self) -> io::Result<ExitStatus> {
        if let Some(e) = refusal(&self.program, NO_PROCESSES) {
            return Err(e);
        }
        self.inner.status()
    }

    pub fn spawn(&mut self) -> io::Result<Child> {
        if let Some(e) = refusal(&self.program, NO_PROCESSES) {
            return Err(e);
        }
        self.inner.spawn()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unavailable_names_the_program() {
        let e = unavailable("ffmpeg");
        assert_eq!(e.kind(), io::ErrorKind::Unsupported);
        assert!(e.to_string().starts_with("ffmpeg: unavailable on this platform"));
    }

    /// Every program the workspace names in a `Command::new("…")` (computed from
    /// the sources, not typed here).
    fn programs() -> std::collections::BTreeSet<String> {
        fn walk(dir: &std::path::Path, out: &mut std::collections::BTreeSet<String>) {
            for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk(&p, out);
                } else if p.extension().is_some_and(|x| x == "rs") {
                    let text = std::fs::read_to_string(&p).unwrap_or_default();
                    for part in text.split("Command::new(\"").skip(1) {
                        if let Some((name, _)) = part.split_once('"') {
                            if !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
                                out.insert(name.to_string());
                            }
                        }
                    }
                }
            }
        }
        let mut out = std::collections::BTreeSet::new();
        walk(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(".."), &mut out);
        out
    }

    /// Android has no child processes: every program in the workspace is refused
    /// there with "unavailable on this platform", and none is refused elsewhere.
    #[test]
    fn every_program_answers_unavailable_on_android() {
        let list = programs();
        assert!(list.contains("ffmpeg"), "the scan found the sources");
        for program in &list {
            let e = refusal(program, true).unwrap_or_else(|| panic!("{program} would spawn on Android"));
            assert_eq!(e.kind(), io::ErrorKind::Unsupported);
            assert!(e.to_string().starts_with(&format!("{program}: unavailable on this platform")), "{e}");
            assert!(refusal(program, false).is_none());
        }
    }

    /// `docs/MULTIPLATFORM.md` lists every program the workspace spawns.
    #[test]
    fn the_multiplatform_doc_lists_every_program() {
        let doc = std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/MULTIPLATFORM.md")).unwrap();
        for program in programs() {
            assert!(doc.contains(&format!("`{program}`")), "docs/MULTIPLATFORM.md does not list `{program}`");
        }
    }

    #[cfg(not(target_os = "android"))]
    #[test]
    fn spawns_where_processes_exist() {
        assert!(Command::new("true").status().is_ok());
    }

    #[cfg(target_os = "android")]
    #[test]
    fn short_circuits_on_android() {
        let e = Command::new("ffmpeg").arg("-version").output().unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::Unsupported);
    }
}
