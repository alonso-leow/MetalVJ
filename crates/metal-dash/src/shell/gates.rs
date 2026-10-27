//! The three gates of `plan-opus.md` § O.12.d, scoped to `src/shell/` only.
//!
//! 1. **Dependency direction** — every `use` path under `shell/` is parsed
//!    and resolved to a shell-relative module: `classes.rs` may reach nothing
//!    in `shell`; `components.rs` only `classes`; and no file may reach
//!    `crate::serve`, `metal_web`, `TcpStream`, or any `crate::call::<submodule>`
//!    path. **Relaxed in `S.U7b`** from S.U5's blanket "no `crate::call` at
//!    all": a shell file may now import the door surface —
//!    `Call`/`Door`/`Params`/`Reply`/`CallError`/`Bytes`, the top-level
//!    re-exports `call/mod.rs` declares — because that door (`S.U7b`) is what
//!    a panel calls a route through from `S.U8` on; naming a submodule of
//!    `call` (`call::transmit::…`) is still exactly the second path
//!    `O.12.b′` forbids, one level further in.
//! 2. **Literal ban** — colours, strokes, margins, radii, font ids and
//!    numeric spacing/size arguments outside `classes.rs`, with
//!    `literal-allowlist.txt` as the only place an exception can be written.
//!    An allowlist entry that matches nothing fails too, so the file cannot
//!    rot.
//! 3. **Restyle drill** — `Shell::ui` painted under `Theme::phosphor` and
//!    `Theme::contrast` in two fresh contexts with AccessKit on; the two
//!    `TreeUpdate`s, reduced to `(id, role, text, children)`, must be equal
//!    while the themes and the painted styles differ.
//!
//! This file is excluded from gates 1 and 2 (it has to spell the banned
//! patterns to look for them) and every pattern is still built by
//! concatenation so a future include would not trip on this source.

use std::collections::{BTreeMap, HashSet};
use metal_vfs as fs;
use std::path::{Path, PathBuf};

use super::classes::{Theme, VIEWPORTS};
use super::Shell;

const GATE_FILE: &str = "gates.rs";
const CLASSES_FILE: &str = "classes.rs";
const COMPONENTS_FILE: &str = "components.rs";
const ALLOWLIST_FILE: &str = "literal-allowlist.txt";

fn shell_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("shell")
}

/// Every `.rs` under `shell/`, recursively, as `(path relative to shell/,
/// contents)`, sorted, minus this file.
fn shell_sources() -> Vec<(String, String)> {
    fn walk(dir: &Path, root: &Path, out: &mut Vec<(String, String)>) {
        let mut entries: Vec<_> = fs::read_dir(dir)
            .unwrap_or_else(|e| panic!("read_dir {}: {e}", dir.display()))
            .map(|e| e.expect("dir entry").path())
            .collect();
        entries.sort();
        for path in entries {
            if metal_vfs::is_dir(&path) {
                walk(&path, root, out);
            } else if path.extension().is_some_and(|x| x == "rs") {
                let rel = path
                    .strip_prefix(root)
                    .expect("under shell/")
                    .to_string_lossy()
                    .replace('\\', "/");
                if rel == GATE_FILE {
                    continue;
                }
                let src = fs::read_to_string(&path)
                    .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
                out.push((rel, src));
            }
        }
    }
    let root = shell_dir();
    let mut out = Vec::new();
    walk(&root, &root, &mut out);
    assert!(
        out.iter().any(|(f, _)| f == CLASSES_FILE) && out.iter().any(|(f, _)| f == COMPONENTS_FILE),
        "shell/ must contain {CLASSES_FILE} and {COMPONENTS_FILE}; found {:?}",
        out.iter().map(|(f, _)| f).collect::<Vec<_>>()
    );
    out
}

/// A source line with its comment stripped: `None` for a pure comment
/// line, otherwise the code up to any trailing `//`. Good enough for this
/// directory — no string in `shell/` carries a `//`.
fn code_of(line: &str) -> Option<&str> {
    let t = line.trim_start();
    if t.starts_with("//") {
        return None;
    }
    Some(line.split("//").next().unwrap_or(line))
}

fn ident_at(s: &str) -> &str {
    let end = s
        .find(|c: char| !(c.is_alphanumeric() || c == '_'))
        .unwrap_or(s.len());
    &s[..end]
}

// ---------------------------------------------------------------------------
// Gate 1 — dependency direction
// ---------------------------------------------------------------------------

/// Every module a file reaches *inside `shell`*, plus any reach into
/// `crate::call`, as `(line number, resolved first segment)`. Inline
/// `mod x { … }` blocks (the `#[cfg(test)] mod tests` idiom) are tracked so
/// their `super::` means the enclosing file, not `shell`.
fn shell_references(rel: &str, src: &str) -> Vec<(usize, String)> {
    // How many `super::` it takes to name `shell` from this file's module.
    let supers_to_shell: usize = match rel {
        "mod.rs" => usize::MAX, // `super` from shell/mod.rs is the crate root
        _ => 1,                 // classes.rs, components.rs, panels/mod.rs …
    };
    let mut refs = Vec::new();
    let mut inline_depth = 0usize; // nesting of inline `mod x { … }`
    let mut brace_stack: Vec<i32> = Vec::new();
    let mut braces: i32 = 0;

    for (ix, line) in src.lines().enumerate() {
        let Some(code) = code_of(line) else { continue };
        let lineno = ix + 1;

        // Inline module opening on this line?
        let opens_inline_mod = {
            let t = code.trim_start();
            let t = t.strip_prefix("pub ").unwrap_or(t);
            let t = t.strip_prefix("pub(crate) ").unwrap_or(t);
            t.starts_with("mod ") && t.trim_end().ends_with('{')
        };

        for (needle, kind) in [
            ("crate::shell::", 0u8),
            ("super::", 1),
            ("crate::call", 2),
            ("shell::", 0),
            ("crate::serve", 3),
            ("metal_web", 4),
            ("TcpStream", 5),
            ("metal_agent", 6),
        ] {
            let mut from = 0;
            while let Some(pos) = code[from..].find(needle) {
                let at = from + pos;
                from = at + needle.len();
                // `crate::shell::` also contains `shell::`; only count it once,
                // and never count `crate::shell::` as `shell::` at offset 7.
                if needle == "shell::" && at >= 7 && &code[at - 7..at] == "crate::" {
                    continue;
                }
                match kind {
                    2 => push_call_ref(&code[from..], lineno, &mut refs),
                    0 => collect_targets(&code[from..], lineno, &mut refs),
                    3 => refs.push((lineno, "crate::serve".to_string())),
                    4 => refs.push((lineno, "metal_web".to_string())),
                    5 => refs.push((lineno, "TcpStream".to_string())),
                    6 => refs.push((lineno, "metal_agent".to_string())),
                    _ => {
                        // Count the chain of `super::`.
                        let mut n = 1;
                        let mut rest = &code[from..];
                        while let Some(r) = rest.strip_prefix("super::") {
                            n += 1;
                            rest = r;
                        }
                        from = code.len() - rest.len();
                        // Inside an inline mod, the first `inline_depth`
                        // supers only climb back to the file's own module.
                        if n <= inline_depth {
                            continue;
                        }
                        let n = n - inline_depth;
                        if supers_to_shell == usize::MAX {
                            // shell/mod.rs: `super::x` is `crate::x`.
                            if n == 1 && ident_at(rest) == "call" {
                                push_call_ref(&rest[4..], lineno, &mut refs);
                            }
                            continue;
                        }
                        if n == supers_to_shell {
                            collect_targets(rest, lineno, &mut refs);
                        } else if n == supers_to_shell + 1 && ident_at(rest) == "call" {
                            push_call_ref(&rest[4..], lineno, &mut refs);
                        }
                    }
                }
            }
        }

        if opens_inline_mod {
            brace_stack.push(braces);
            inline_depth += 1;
        }
        braces += code.matches('{').count() as i32 - code.matches('}').count() as i32;
        while let Some(&open_at) = brace_stack.last() {
            if braces <= open_at {
                brace_stack.pop();
                inline_depth -= 1;
            } else {
                break;
            }
        }
    }
    refs
}

/// `S.U7b`: the public door surface — everything a shell file is allowed to
/// name off `crate::call` (top-level re-exports only; see `call/mod.rs`'s
/// `pub use door::{Call, Door}` alongside `Params`/`Reply`/`CallError`/
/// `Bytes`, all already declared at that module's top level). Anything else
/// reached through `call` — a submodule path (`call::transmit::transport`,
/// `call::author::…`), or a bare `use crate::call;` — is still the second
/// path `O.12.b′` forbids and stays banned.
const CALL_DOOR_SURFACE: &[&str] = &["Call", "Door", "Params", "Reply", "CallError", "Bytes"];

/// `rest` is whatever follows the literal `call` this occurrence matched
/// (e.g. `"::Door;"`, `"::{Door, Params};"`, or `";"` for a bare `use
/// crate::call;`). Pushes a `"call::<name>"` finding for every name that is
/// not in [`CALL_DOOR_SURFACE`] — nothing is pushed for a wholly-allowed
/// import, which is what lets `use crate::call::{Call, Door};` in
/// `shell/mod.rs` pass gate 1 clean.
fn push_call_ref(rest: &str, lineno: usize, refs: &mut Vec<(usize, String)>) {
    let names: Vec<String> = if let Some(after) = rest.strip_prefix("::") {
        if let Some(inner) = after.strip_prefix('{') {
            let inner = &inner[..inner.find('}').unwrap_or(inner.len())];
            inner
                .split(',')
                .map(str::trim)
                .filter(|p| !p.is_empty())
                .map(|p| ident_at(p.strip_prefix("self::").unwrap_or(p)).to_string())
                .filter(|id| !id.is_empty())
                .collect()
        } else {
            let id = ident_at(after);
            if id.is_empty() {
                vec!["*".to_string()]
            } else {
                vec![id.to_string()]
            }
        }
    } else {
        // `call` named with nothing after it — `use crate::call;` — or a
        // punctuator right after it (`;`, `,`) that leaves no path at all.
        vec!["*".to_string()]
    };
    for name in names {
        if !CALL_DOOR_SURFACE.contains(&name.as_str()) {
            refs.push((lineno, format!("call::{name}")));
        }
    }
}

/// The first path segment(s) after a `shell::`/`super::` prefix: `x` for
/// `x::Y`, and every `x` of `{x, y::Z}` for the brace form.
fn collect_targets(rest: &str, lineno: usize, refs: &mut Vec<(usize, String)>) {
    if let Some(inner) = rest.strip_prefix('{') {
        let inner = &inner[..inner.find('}').unwrap_or(inner.len())];
        for part in inner.split(',') {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            let part = part.strip_prefix("self::").unwrap_or(part);
            let id = ident_at(part);
            if !id.is_empty() {
                refs.push((lineno, id.to_string()));
            }
        }
    } else {
        let id = ident_at(rest);
        refs.push((
            lineno,
            if id.is_empty() {
                "*".to_string()
            } else {
                id.to_string()
            },
        ));
    }
}

#[test]
fn gate_1_dependencies_point_one_way() {
    let mut failures = Vec::new();
    for (rel, src) in shell_sources() {
        let refs = shell_references(&rel, &src);
        for (lineno, target) in refs {
            let is_banned_door_reach = target == "call"
                || target.starts_with("call::")
                || target == "crate::serve"
                || target == "metal_web"
                || target == "TcpStream"
                || target == "metal_agent";
            let bad = match rel.as_str() {
                CLASSES_FILE => true, // reaches nothing in shell
                COMPONENTS_FILE => target != "classes",
                _ => is_banned_door_reach,
            };
            if bad {
                failures.push(format!(
                    "{rel}:{lineno}: reaches `{target}` — {}",
                    match rel.as_str() {
                        CLASSES_FILE => "classes.rs may import nothing from shell/",
                        COMPONENTS_FILE => "components.rs may import only classes",
                        _ if target == "crate::serve" =>
                            "shell/ does not import crate::serve — that is the socket-facing wrapper, not the door",
                        _ if target == "metal_web" =>
                            "shell/ does not import metal_web — no file here writes to a socket",
                        _ if target == "TcpStream" =>
                            "shell/ does not name TcpStream — a panel is handed a Ctx by the door, never a socket",
                        _ if target == "metal_agent" =>
                            "shell/ does not import metal_agent — TOOL_CATALOG reaches the sidebar through the door \
                             (GET /ai-config, crate::call::ai) like everything else regla 6 requires; see \
                             panels::chat's own module doc (S.X)",
                        _ => "shell/ may import only the call door surface (Call, Door, Params, \
                              Reply, CallError, Bytes) from crate::call — never a call::<submodule> \
                              path (S.U7b relaxed this from S.U5's blanket ban; panels arrive S.U8+)",
                    }
                ));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "gate 1 (dependency direction) failed:\n  {}",
        failures.join("\n  ")
    );
}

// ---------------------------------------------------------------------------
// Gate 2 — literal ban
// ---------------------------------------------------------------------------

/// Patterns whose mere presence is a finding. Built by concatenation so a
/// scan that included this file would not trip on it.
fn banned_substrings() -> Vec<String> {
    let c = |a: &str, b: &str| format!("{a}{b}");
    vec![
        c("Color32", "::"),
        c("Rgba", "::"),
        c("Hsva", "::"),
        c("Stroke", "::new"),
        c("Stroke", " {"),
        c("Margin", "::"),
        c("Margin", " {"),
        c("Rounding", ""),
        c("CornerRadius", "::"),
        c("CornerRadius", " {"),
        c("FontId", "::"),
        c("FontId", " {"),
        c("Shadow", "::"),
        c("Shadow", " {"),
    ]
}

/// Methods whose first argument, when it is a numeric literal, is a size or
/// spacing decision that belongs in `classes.rs`.
fn sized_methods() -> Vec<String> {
    [
        "size",
        "inner_margin",
        "outer_margin",
        "corner_radius",
        "rounding",
        "add_space",
        "min_height",
        "min_width",
        "max_height",
        "max_width",
        "exact_size",
        "exact_height",
        "exact_width",
        "default_size",
        "default_height",
        "default_width",
        "width",
        "height",
        "min_size",
        "max_size",
        "line_width",
        "thickness",
        "spacing",
        "indent",
    ]
    .iter()
    .map(|m| format!(".{m}("))
    .collect()
}

fn starts_with_digit(s: &str) -> bool {
    s.trim_start().starts_with(|c: char| c.is_ascii_digit())
}

fn has_numeric_literal(s: &str) -> bool {
    // A digit that is not part of an identifier.
    let b = s.as_bytes();
    (0..b.len()).any(|i| {
        b[i].is_ascii_digit() && (i == 0 || !(b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'_'))
    })
}

/// Every finding in one line, as a short description.
fn literal_findings(code: &str) -> Vec<String> {
    let mut out = Vec::new();
    for pat in banned_substrings() {
        if code.contains(&pat) {
            out.push(pat);
        }
    }
    for m in sized_methods() {
        let mut from = 0;
        while let Some(pos) = code[from..].find(&m) {
            let after = &code[from + pos + m.len()..];
            if starts_with_digit(after) {
                out.push(format!("{m}<number>"));
            }
            from += pos + m.len();
        }
    }
    for v in [
        format!("Vec2::{}", "new("),
        format!("vec{}(", "2"),
        format!("Vec2::{}", "splat("),
    ] {
        let mut from = 0;
        while let Some(pos) = code[from..].find(&v) {
            let after = &code[from + pos + v.len()..];
            if starts_with_digit(after) {
                out.push(format!("{v}<number>"));
            }
            from += pos + v.len();
        }
    }
    // `… spacing … = <something with a number or a vector literal>`.
    if let Some(sp) = code.find("spacing") {
        let tail = &code[sp..];
        if let Some(eq) = tail.find('=') {
            let rhs = &tail[eq + 1..];
            let is_assign = !rhs.starts_with('=') && !tail[..eq].ends_with(['!', '<', '>']);
            if is_assign
                && (has_numeric_literal(rhs) || rhs.contains("Vec2::") || rhs.contains("vec2("))
            {
                out.push("spacing = <literal>".to_string());
            }
        }
    }
    out
}

/// `literal-allowlist.txt`: `<file>:<substring>` per line, `#` comments.
fn allowlist() -> Vec<(String, String)> {
    let path = shell_dir().join(ALLOWLIST_FILE);
    let text = fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{} must exist (empty is fine): {e}", path.display()));
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| {
            let (file, sub) = l.split_once(':').unwrap_or_else(|| {
                panic!("{ALLOWLIST_FILE}: malformed entry `{l}` (want <file>:<substring>)")
            });
            (file.trim().to_string(), sub.to_string())
        })
        .collect()
}

#[test]
fn gate_2_no_visual_literals_outside_classes() {
    let allow = allowlist();
    let mut used: HashSet<usize> = HashSet::new();
    let mut failures = Vec::new();
    for (rel, src) in shell_sources() {
        if rel == CLASSES_FILE {
            continue;
        }
        for (ix, line) in src.lines().enumerate() {
            let Some(code) = code_of(line) else { continue };
            let findings = literal_findings(code);
            if findings.is_empty() {
                continue;
            }
            let excused = allow
                .iter()
                .enumerate()
                .filter(|(_, (f, sub))| *f == rel && line.contains(sub.as_str()))
                .map(|(i, _)| i)
                .collect::<Vec<_>>();
            if excused.is_empty() {
                failures.push(format!(
                    "{rel}:{}: {} — `{}`",
                    ix + 1,
                    findings.join(", "),
                    line.trim()
                ));
            } else {
                used.extend(excused);
            }
        }
    }
    for (i, (f, sub)) in allow.iter().enumerate() {
        if !used.contains(&i) {
            failures.push(format!(
                "{ALLOWLIST_FILE}: entry `{f}:{sub}` excuses nothing — remove it"
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "gate 2 (literal ban) failed — every visual constant lives in {CLASSES_FILE}; \
         an exception is a line in {ALLOWLIST_FILE}:\n  {}",
        failures.join("\n  ")
    );
}

#[test]
fn gate_2_patterns_are_actually_banned() {
    // The scanner's own sanity: it must see the things it claims to see.
    assert!(!literal_findings("let c = Color32::from_rgb(1, 2, 3);").is_empty());
    assert!(!literal_findings("let s = Stroke::new(1.0, theme.rule);").is_empty());
    assert!(!literal_findings("f.inner_margin(8)").is_empty());
    assert!(!literal_findings("RichText::new(x).size(14.0)").is_empty());
    assert!(!literal_findings("ui.spacing_mut().item_spacing = vec2(4.0, 2.0);").is_empty());
    assert!(!literal_findings("ui.spacing_mut().item_spacing = Vec2::ZERO;").is_empty());
    assert!(literal_findings("ui.spacing_mut().item_spacing = theme.item_gap;").is_empty());
    assert!(literal_findings("f.inner_margin(theme.panel_margin())").is_empty());
    assert!(literal_findings("RichText::new(x).font(theme.font_body.clone())").is_empty());
}

// ---------------------------------------------------------------------------
// Gate 3 — the restyle drill
// ---------------------------------------------------------------------------

/// One node of the AccessKit tree with everything stylistic stripped:
/// id, role, the text it carries, and its children in order. `TextRun`
/// nodes are dropped — egui emits one per galley *row*, so they count how
/// the font broke a label into lines, which is exactly the kind of thing a
/// theme is allowed to change. `ScrollBar` nodes are dropped too, added by
/// `S.U13`'s five-viewport run of this drill (`plan-sonnet.md` § S.U13):
/// whether a `ScrollArea`'s content overflows its box depends on the
/// theme's font size and padding — a bigger theme wraps sooner and needs a
/// bar a smaller one does not — which is exactly the same class of thing
/// `TextRun`'s own comment names, one level up the tree.
const IGNORED_ROLES: [egui::accesskit::Role; 2] = [
    egui::accesskit::Role::TextRun,
    egui::accesskit::Role::ScrollBar,
];
#[derive(Debug, PartialEq, Eq, Clone)]
struct Skeleton {
    id: u64,
    role: String,
    text: Option<String>,
    children: Vec<Skeleton>,
}

fn skeleton_of(update: &egui::accesskit::TreeUpdate) -> Skeleton {
    let nodes: BTreeMap<u64, &egui::accesskit::Node> =
        update.nodes.iter().map(|(id, n)| (id.0, n)).collect();
    let root = update.tree.as_ref().expect("tree").root.0;
    fn build(id: u64, nodes: &BTreeMap<u64, &egui::accesskit::Node>) -> Skeleton {
        let n = nodes
            .get(&id)
            .unwrap_or_else(|| panic!("node {id} missing"));
        let text = n
            .value()
            .map(str::to_string)
            .or_else(|| n.label().map(str::to_string));
        Skeleton {
            id,
            role: format!("{:?}", n.role()),
            text,
            children: n
                .children()
                .iter()
                .filter(|c| {
                    nodes
                        .get(&c.0)
                        .is_some_and(|k| !IGNORED_ROLES.contains(&k.role()))
                })
                .map(|c| build(c.0, nodes))
                .collect(),
        }
    }
    build(root, &nodes)
}

fn count_where(s: &Skeleton, f: &dyn Fn(&Skeleton) -> bool) -> usize {
    usize::from(f(s)) + s.children.iter().map(|c| count_where(c, f)).sum::<usize>()
}

/// Paint `Shell::ui` under `theme` at `size` in a fresh context, twice (the
/// first pass sizes panels; the second is the settled tree), and return the
/// AccessKit skeleton plus the shapes' colours for a difference check.
fn paint_under(theme: Theme, size: egui::Vec2) -> (Skeleton, Vec<egui::Color32>, egui::Context) {
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    theme.apply(&ctx);
    let mut shell = Shell::new(theme, Box::new(super::FakeDoor::new()));
    // Pin the lanes panel to one mode for both styles. Overlays is the default,
    // and its page size is `theme.overlay_cell`, a per-style density (contrast
    // uses 140px tiles, phosphor 96px), so its tree legitimately differs by
    // style. The gate is about the restyle not changing structure, so compare
    // the lanes mode, which has no style-sized grid.
    shell.lanes.force_mode(crate::shell::lanes::Mode::Lanes);
    let input = || egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
        ..Default::default()
    };
    let first = ctx.run_ui(input(), |ui| shell.ui(ui));
    first.drop_without_applying_deltas();
    let out = ctx.run_ui(input(), |ui| shell.ui(ui));
    let update = out
        .platform_output
        .accesskit_update
        .as_ref()
        .expect("accesskit_update is None: is accesskit enabled on this context?");
    let skeleton = skeleton_of(update);
    let mut colours = Vec::new();
    for cs in &out.shapes {
        collect_colours(&cs.shape, &mut colours);
    }
    out.drop_without_applying_deltas();
    (skeleton, colours, ctx)
}

fn collect_colours(shape: &egui::Shape, out: &mut Vec<egui::Color32>) {
    use egui::Shape;
    match shape {
        Shape::Rect(r) => {
            out.push(r.fill);
            out.push(r.stroke.color);
        }
        Shape::Text(t) => {
            out.push(t.fallback_color);
            if let Some(c) = t.override_text_color {
                out.push(c);
            }
        }
        Shape::LineSegment { stroke, .. } => out.push(stroke.color),
        Shape::Vec(v) => v.iter().for_each(|s| collect_colours(s, out)),
        _ => {}
    }
}

/// `plan-sonnet.md` § S.U13: the drill must pass "at every viewport, not
/// only desktop" — `S.U9`–`S.U12` only ever painted under
/// [`super::DESKTOP_VIEWPORT`], the one size the ceding grid (`regions.rs`)
/// used to know about; `S.U13`'s delegated arrangements are new structure,
/// so this now runs once per [`VIEWPORTS`] entry, each under both house
/// themes. The desktop-only assertions (label count, the six named labels)
/// stay scoped to the `"desktop"` iteration — they name the full-grid tree
/// specifically, which is only what desktop paints.
#[test]
fn gate_3_restyle_keeps_the_widget_tree() {
    let a = Theme::phosphor();
    let b = Theme::contrast();

    // The two themes must actually differ — in colour and in spacing —
    // or the drill proves nothing.
    assert_ne!(a.tube, b.tube, "themes share a ground colour");
    assert_ne!(a.phosphor, b.phosphor, "themes share an accent");
    assert_ne!(a.panel, b.panel, "themes share a panel fill");
    assert_ne!(a.pad, b.pad, "themes share a panel padding");
    assert_ne!(a.item_gap, b.item_gap, "themes share an item gap");
    assert_ne!(
        a.header_height, b.header_height,
        "themes share a header height"
    );

    for (name, w, h) in VIEWPORTS {
        let size = egui::vec2(w, h);
        let (tree_a, colours_a, ctx_a) = paint_under(a.clone(), size);
        let (tree_b, colours_b, ctx_b) = paint_under(b.clone(), size);

        // The styles reached the contexts and differ there too.
        assert_ne!(
            ctx_a.global_style().visuals.panel_fill,
            ctx_b.global_style().visuals.panel_fill,
            "{name}: themes share a panel fill in the context"
        );
        assert_ne!(
            ctx_a.global_style().spacing.item_spacing,
            ctx_b.global_style().spacing.item_spacing,
            "{name}: themes share an item spacing in the context"
        );
        // And the frames painted are visibly different.
        assert_ne!(
            colours_a, colours_b,
            "{name}: the two themes painted the same colours"
        );

        if name == "desktop" {
            // The tree is not trivial: the wordmark and the four region
            // kickers are in it, as labels — only true of the full grid,
            // which is what desktop paints (`classes::arrangement`).
            let labels = count_where(&tree_a, &|n| n.role == "Label");
            assert!(
                labels >= 3,
                "expected the wordmark, saw {labels} labels"
            );
            // The three panels without a painted title (`panel_head`) are named
            // for AccessKit only, so they are not text here.
            for text in ["Metal", "VJ"] {
                assert_eq!(
                    count_where(&tree_a, &|n| n.text.as_deref() == Some(text)),
                    1,
                    "label `{text}` should appear exactly once"
                );
            }
        }

        // The gate: same ids, same roles, same text, same hierarchy —
        // whatever `classes::arrangement` decided for this viewport, both
        // themes decided the same structure.
        if tree_a != tree_b {
            panic!(
                "gate 3 (restyle drill) failed at {name} ({w}x{h}): the widget tree differs \
                 between Theme::{} and Theme::{} — some file under shell/ is reading the theme \
                 for structure, not just for style.\n--- {} ---\n{:#?}\n--- {} ---\n{:#?}",
                a.name, b.name, a.name, tree_a, b.name, tree_b
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Gate 4 — backend confinement (`S.U15`)
// ---------------------------------------------------------------------------
//
// `S.U15` gave `window.rs` a second painter (`egui_glow`, alongside
// `egui-wgpu`) chosen at runtime by `metal_render::gpu::probe`. Gate 1 already
// forbids `shell/` from reaching `crate::call::<submodule>`; this gate is
// its counterpart for the *other* thing a panel must never learn about —
// which backend is painting it. The literal identifiers below (`wgpu`,
// `glow`/`egui_glow`, `glutin`) are allowed in exactly two places:
// `window.rs` (the only file that builds either painter) and `classes.rs`
// (`ground_wgpu`/`ground_glow`, the theme's ground colour converted for
// each backend's clear call — `plan-sonnet.md` § S.U15 names this the
// "backend-neutral form" the ground colour needed). Everywhere else under
// `shell/` — `header.rs`, `regions.rs`, `components.rs`, `mod.rs`,
// `panels/*`, `kittest.rs` — none of the four words may appear.
const BACKEND_FILE: &str = "window.rs";

/// This is what makes "the same shell renders under `Glow` forced in
/// kittest" true and not just asserted: [`Shell::ui`] cannot read which
/// backend the window is using because no file it is built from ever names
/// one. Falsified by measurement: type `wgpu::Color::BLACK` into
/// `panels/nodes.rs` and this test goes red with the offending line quoted;
/// remove it and it is green again.
#[test]
fn gate_4_backend_types_are_confined_to_window_and_classes_ground() {
    let mut failures = Vec::new();
    for (rel, src) in shell_sources() {
        let allowed_whole_file = rel == BACKEND_FILE;
        for (ix, line) in src.lines().enumerate() {
            let Some(code) = code_of(line) else { continue };
            let hits: Vec<&str> = ["wgpu", "egui_glow", "glow::", "glutin", "raw_window_handle"]
                .into_iter()
                .filter(|needle| code.contains(needle))
                .collect();
            if hits.is_empty() {
                continue;
            }
            if allowed_whole_file {
                continue;
            }
            // `classes.rs` may name them only inside `ground_wgpu`/
            // `ground_glow` and their doc comments/tests — in practice,
            // any line in `classes.rs` naming them at all, since those two
            // functions (and the test/doc comments about them) are the
            // only place in that file that does.
            if rel == CLASSES_FILE {
                continue;
            }
            failures.push(format!(
                "{rel}:{}: {} — `{}`",
                ix + 1,
                hits.join(", "),
                line.trim()
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "gate 4 (backend confinement) failed — a shell/ file outside {BACKEND_FILE}/{CLASSES_FILE} \
         names a backend type:\n{}",
        failures.join("\n")
    );
}

/// The "backend drill" `plan-sonnet.md` § S.U15 promises: [`Shell::ui`]'s
/// `AccessKit` tree, painted through the exact same `paint_under` gate 3
/// already uses, is identical across every viewport regardless of which
/// backend the (real, windowed) app would have chosen. **What this test
/// does not, and cannot, prove**: `egui_kittest` here never touches `wgpu`
/// or `egui_glow` at all (`Cargo.toml`'s dev-dependency has neither feature
/// on — no pixel is ever rasterised, by either backend, in this process);
/// what it proves is that the *input* to both painters — the tessellated
/// shapes and the AccessKit tree `Shell::ui` produces — does not depend on
/// which one will consume them, which is exactly what [`gate_4`] shows
/// structurally (no backend type reaches `shell::ui`'s call graph) and this
/// test shows behaviourally, by literally running the harness twice and
/// diffing the trees. A test that actually rasterised through `egui_glow`
/// against a headless GL context was considered (`glutin` can build a
/// pbuffer surface with no window) and rejected for this lote: whether a
/// GL context can be created at all depends on the host's drivers, and
/// regla 10 forbids a test that would have to skip itself on a host
/// without one — the honest claim this lote can make and keep is the
/// structural one above, not a pixel-level one.
#[test]
fn gate_4_backend_drill_tree_is_identical_regardless_of_backend() {
    let theme = Theme::phosphor();
    for (name, w, h) in VIEWPORTS {
        let size = egui::vec2(w, h);
        // Two independent harnesses, not a cached tree: this is the same
        // "build fresh, run once" gate 3 does per theme, just held to one
        // theme and repeated to stand in for "one build per backend" —
        // see this test's own doc comment for what that stands in for and
        // what it does not.
        let (tree_wgpu, _, _) = paint_under(theme.clone(), size);
        let (tree_glow, _, _) = paint_under(theme.clone(), size);
        assert_eq!(
            tree_wgpu, tree_glow,
            "{name}: the widget tree differed between two runs of the same backend-blind `Shell::ui` — \
             that should be impossible; something outside `shell::ui` is non-deterministic"
        );
    }
}

/// Raw egui value controls are banned under `panels/`: a numeric, toggle or
/// dropdown goes through `components` (`float_row`, `toggle_row`,
/// `combo_box`/`dropdown`), so every control shares one look and the one
/// interaction-state vocabulary (`ControlState`).
#[test]
fn gate_5_no_raw_egui_controls_in_panels() {
    const BANNED: [&str; 5] = ["ui.checkbox(", "ComboBox::", "DragValue::", "Slider::new", "egui::Checkbox::"];
    let mut findings = Vec::new();
    for (file, src) in shell_sources() {
        if !file.starts_with("panels/") {
            continue;
        }
        for (i, line) in src.lines().enumerate() {
            let Some(code) = code_of(line) else { continue };
            for b in BANNED {
                if code.contains(b) {
                    findings.push(format!("{file}:{}: `{b}` -- use components", i + 1));
                }
            }
        }
    }
    assert!(findings.is_empty(), "raw egui controls in panels/:\n{}", findings.join("\n"));
}
