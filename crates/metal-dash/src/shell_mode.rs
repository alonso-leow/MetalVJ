//! The shell mode vocabulary: one enum, one wire spelling, shared by the
//! file's route and the shell that consumes it.
//!
//! **Top-level on purpose** (`LAY.10`). `shell/classes.rs` is a leaf that
//! gate 1 lets reach nothing in `shell/`, and `call/settings.rs` is the route
//! that owns `data/settings.json` — including on a build with no window,
//! where it must still refuse a mode it does not know rather than store it.
//! With the enum in `shell::classes` the route could only validate by
//! reaching into the shell (banned by the gate, and uncompiled on a
//! no-window build); with the vocabulary here, both read one definition and
//! neither depends on the other.

/// The shell mode the settings overlay's dropdown writes (`O.16`'s first
/// slice, shared with `call::settings`, `S.AF`). **`Auto` is a third
/// state**, not a missing one: the viewport decision of
/// [`crate::shell::classes::arrangement`] stays the default —
/// "mobile"/"desktop" are the manual overrides the operator of the actual
/// device is owed (`plan-sonnet.md` § S.AF), for the case the arrangement's
/// thresholds would pick the wrong half — a tablet folded into its stand, or
/// a very wide phone — which no threshold can ever see.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShellMode {
    /// What `arrangement` decides from the live viewport — the shipped
    /// default and the value an untouched `data/settings.json` loads.
    Auto,
    /// Manual override: the full `S.U8`–`S.U12` grid, whatever the window
    /// measures. The tablet strapped to a tripod case: the operator knows
    /// the size, the thresholds don't.
    Escritorio,
    /// Manual override: everything delegated — library sidebar, one
    /// header tab, long-press properties, overlay harness — whatever the
    /// window measures.
    Movil,
}

impl ShellMode {
    /// The wire vocabulary, in [`ShellMode::wire`]'s own order — the file
    /// format, not the UI. `parse` and `wire` both walk this one array, so
    /// the two can never spell a mode differently, and the route validates
    /// against it without importing the shell.
    pub const WIRE: [&'static str; 3] = ["auto", "desktop", "mobile"];

    /// Parses the `data/settings.json` value (and `call::settings::post`'s
    /// body). `None` for anything else — a typo is refused at the route
    /// (`call::settings::post_at`), not silently read as the default here.
    /// The pre-English spellings `data/settings.json` may still hold
    /// (`escritorio`, `movil`) read as the same modes; they are never written.
    pub fn parse(s: &str) -> Option<Self> {
        let s = match s {
            "escritorio" => "desktop",
            "movil" => "mobile",
            other => other,
        };
        Some(match ShellMode::WIRE.iter().position(|w| *w == s)? {
            0 => ShellMode::Auto,
            1 => ShellMode::Escritorio,
            _ => ShellMode::Movil,
        })
    }

    /// The whole dropdown, order fixed: the default first, then the two
    /// manual overrides — the panel's picker walks this array rather than
    /// writing the words down a second time (regla 7 for a vocabulary).
    pub const ALL: [(&'static str, ShellMode); 3] = [
        ("automatic", ShellMode::Auto),
        ("desktop", ShellMode::Escritorio),
        ("mobile", ShellMode::Movil),
    ];

    /// The wire value this mode writes into `data/settings.json` (and
    /// [`ShellMode::parse`] reads back). One vocabulary, two spellings of
    /// the same three strings is already too many: the picker never spells
    /// these a second time, and the route refuses anything `parse` doesn't
    /// know.
    pub fn wire(self) -> &'static str {
        ShellMode::WIRE[match self {
            ShellMode::Auto => 0,
            ShellMode::Escritorio => 1,
            ShellMode::Movil => 2,
        }]
    }
}

#[cfg(test)]
mod tests {
    use super::ShellMode;

    #[test]
    fn a_settings_file_written_with_the_spanish_mode_names_still_loads() {
        assert_eq!(ShellMode::parse("escritorio"), Some(ShellMode::Escritorio));
        assert_eq!(ShellMode::parse("movil"), Some(ShellMode::Movil));
        assert_eq!(ShellMode::Movil.wire(), "mobile");
        assert_eq!(ShellMode::Escritorio.wire(), "desktop");
    }
}
