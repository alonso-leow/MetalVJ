//! Display names and search keywords for node tokens (`audio.file`,
//! `gen-8`, …). A node's internal token carries two levels — the family and
//! the leaf — that an operator should never read raw; [`display_name`] hides
//! them behind the one label a card shows, and [`keywords`] gives a picker
//! enough spellings to find a kind by what it does instead of what it is
//! called. The family map is also where the folder label's trailing colon
//! lives, so `gen` says `Animation:` without the function body knowing gen.

const FAMILIES: &[(&str, &str, bool)] = &[
    ("gen", "Animation", true),
    ("audio", "Audio", false),
    ("fx", "Effect", false),
    ("mat", "Material", false),
    ("source", "Source", false),
    ("out", "Output", false),
    ("venue", "Venue", false),
    ("reel", "Reel", false),
    ("mask", "Mask", false),
    ("ai", "AI", false),
    ("compose", "Compose", false),
    ("stack", "Stack", false),
    ("export", "Export", false),
    ("script", "Script", false),
    ("sense", "Sensor", false),
];

/// The display name hidden in an internal token.
pub(crate) fn display_name(token: &str) -> String {
    if let Some(idx) = token.find('.') {
        return leaf_display(&token[idx + 1..]);
    }
    if let Some(family) = instance_family(token) {
        return folder_label(&family);
    }
    token.to_string()
}

/// The family's own display label (`gen` → `Animation`, `audio` → `Audio`),
/// used where a card's second line names the family rather than the leaf.
pub(crate) fn family_name(family: &str) -> String {
    family_label(family).0
}

/// Ten-ish search keywords for a node kind (or instance token).
pub(crate) fn keywords(token: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    push_unique(&mut out, token);
    push_unique(&mut out, &display_name(token));

    let (family, leaf) = split_segments(token);

    if let Some(family) = family.as_deref() {
        push_unique(&mut out, family);
        let (label, _) = family_label(family);
        push_unique(&mut out, &label);
        for synonym in family_synonyms(family) {
            push_unique(&mut out, synonym);
        }
    }
    if let Some(leaf) = leaf.as_deref() {
        push_unique(&mut out, leaf);
        for synonym in leaf_synonyms(leaf) {
            push_unique(&mut out, synonym);
        }
    }

    let base = family.unwrap_or_else(|| token.to_string());
    let mut n = 0;
    while out.len() < 10 && n < 64 {
        let candidate = match n {
            0 => format!("{base} node"),
            1 => format!("{base} model"),
            2 => format!("{base} kind"),
            3 => format!("{base} family"),
            _ => format!("{base} {n}"),
        };
        push_unique(&mut out, &candidate);
        n += 1;
    }
    out
}

fn push_unique(out: &mut Vec<String>, raw: &str) {
    let value = raw.trim().to_lowercase();
    if !value.is_empty() && !out.iter().any(|s| *s == value) {
        out.push(value);
    }
}

fn split_segments(token: &str) -> (Option<String>, Option<String>) {
    if let Some(idx) = token.find('.') {
        return (non_empty(&token[..idx]), non_empty(&token[idx + 1..]));
    }
    (instance_family(token), None)
}

fn non_empty(s: &str) -> Option<String> {
    if s.is_empty() {
        None
    } else {
        Some(s.to_string())
    }
}

fn instance_family(token: &str) -> Option<String> {
    let idx = token.rfind('-')?;
    let family = &token[..idx];
    let suffix = &token[idx + 1..];
    if family.is_empty() || suffix.is_empty() {
        return None;
    }
    if !family.chars().all(|c| c.is_ascii_alphanumeric()) {
        return None;
    }
    if !suffix.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    Some(family.to_string())
}

fn leaf_display(leaf: &str) -> String {
    match leaf {
        "live" => "Device".to_string(),
        "file" => "File".to_string(),
        _ => title_case(leaf),
    }
}

fn folder_label(family: &str) -> String {
    let (label, colon) = family_label(family);
    if colon {
        format!("{label}:")
    } else {
        label
    }
}

fn family_label(family: &str) -> (String, bool) {
    for (key, label, colon) in FAMILIES {
        if *key == family {
            return ((*label).to_string(), *colon);
        }
    }
    (title_case(family), false)
}

fn title_case(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

fn family_synonyms(family: &str) -> &'static [&'static str] {
    match family {
        "audio" => &["source", "media", "sound", "sample", "clip", "track", "input", "device", "music"],
        "gen" => &["generator", "animation", "ai", "model", "synthetic", "generate"],
        "fx" => &["effect", "filter", "processing", "shader", "image", "video", "imagefx"],
        "mat" => &["material", "surface", "shader", "texture", "look"],
        "source" => &["input", "media", "file", "clip", "footage", "video"],
        "out" => &["output", "device", "monitor", "dmx", "sacn", "artnet", "display", "send"],
        "venue" => &["walls", "projection", "stage", "space", "room"],
        "reel" => &["clip", "video", "sequence", "footage", "playlist", "media"],
        "mask" => &["matte", "shape", "alpha", "cutout", "region"],
        "ai" => &["model", "prompt", "neural", "inference", "agent"],
        "compose" => &["layer", "merge", "mix", "blend", "composite"],
        "stack" => &["layer", "group", "chain", "batch"],
        "export" => &["output", "save", "render", "encode", "write"],
        "script" => &["code", "logic", "expression", "program"],
        "sense" => &["sensor", "input", "signal", "trigger", "detect"],
        _ => &[],
    }
}

fn leaf_synonyms(leaf: &str) -> &'static [&'static str] {
    match leaf {
        "file" => &["source", "media", "sound", "sample", "clip", "track", "input", "audio"],
        "live" => &["device", "input", "capture", "audio", "source", "signal"],
        "glitch" => &["effect", "fx", "distort", "noise", "glitchy", "filter"],
        "sacn" => &["dmx", "output", "artnet", "lighting", "universe", "network"],
        _ => &[],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(token: &str) -> std::collections::HashSet<String> {
        keywords(token).into_iter().collect()
    }

    #[test]
    fn display_name_examples() {
        assert_eq!(display_name("audio.file"), "File");
        assert_eq!(display_name("audio.live"), "Device");
        assert_eq!(display_name("gen.mengano"), "Mengano");
        assert_eq!(display_name("gen-8"), "Animation:");
        assert_eq!(display_name("audio-2"), "Audio");
    }

    #[test]
    fn display_name_other_families() {
        assert_eq!(display_name("fx.glitch"), "Glitch");
        assert_eq!(display_name("out.sacn"), "Sacn");
        assert_eq!(display_name("mask-3"), "Mask");
        assert_eq!(display_name("unknown.thing"), "Thing");
        assert_eq!(display_name("plain"), "plain");
        assert_eq!(display_name("not-an-instance"), "not-an-instance");
    }

    fn assert_keywords(token: &str) {
        let list = keywords(token);
        assert!(list.len() >= 10, "{token} only yielded {:?}", list);
        for keyword in &list {
            assert_eq!(*keyword, keyword.to_lowercase(), "{token} keyword not lowercased: {keyword}");
        }
        let unique = set(token);
        assert_eq!(unique.len(), list.len(), "{token} keywords not unique: {list:?}");
    }

    #[test]
    fn keywords_are_ten_unique_lowercased() {
        for token in ["audio.file", "audio.live", "gen.mengano", "gen-8", "fx.glitch", "out.sacn"] {
            assert_keywords(token);
        }
    }

    #[test]
    fn keywords_cover_expected_synonyms() {
        let audio_file = set("audio.file");
        for expected in ["audio", "file", "device", "source", "media", "sound", "sample", "clip", "track", "input"] {
            assert!(audio_file.contains(expected), "audio.file missing {expected}");
        }

        let gen_mengano = set("gen.mengano");
        for expected in ["animation", "generator", "gen", "mengano"] {
            assert!(gen_mengano.contains(expected), "gen.mengano missing {expected}");
        }
    }
}
