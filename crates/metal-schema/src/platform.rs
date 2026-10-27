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
        if NO_PROCESSES {
            return Err(unavailable(&self.program));
        }
        self.inner.output()
    }

    pub fn status(&mut self) -> io::Result<ExitStatus> {
        if NO_PROCESSES {
            return Err(unavailable(&self.program));
        }
        self.inner.status()
    }

    pub fn spawn(&mut self) -> io::Result<Child> {
        if NO_PROCESSES {
            return Err(unavailable(&self.program));
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
