//! The `@` completion list of the chat composer, as pure data: which token the
//! caret is inside, how the candidates rank against what was typed, and how a
//! chosen candidate splices back into the prompt. No egui and no door here, so
//! the ranking is testable on its own; `chat.rs` draws the list and feeds it.
//!
//! The ranking is three signals, kept apart on purpose: a fuzzy match on the
//! typed query (a candidate that does not contain the query's characters in
//! order is out), then the focus (the element the operator has selected sorts
//! first, whatever it matches), then recency (how often this session already
//! inserted it). Focus outranks recency so the list answers "what am I looking
//! at" before "what did I use last".

/// What a candidate names. Kept for the row's glyph and for tests; the token
/// text is what gets inserted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A node of the open graph: its id is the token.
    Node,
    /// One property of a node: `node@property`.
    Property,
    /// A file in the library.
    File,
    /// An overlay on the overlays grid.
    Overlay,
}

/// One thing `@` can name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    /// What is inserted after the `@`, e.g. `gen-1` or `gen-1@opacity`.
    pub token: String,
    /// What the row shows: the name a person would recognise.
    pub label: String,
    pub kind: Kind,
    /// Whether this is the element the operator has selected.
    pub focused: bool,
    /// How many times this session already inserted the token.
    pub recent: u32,
}

/// An `@` token under the caret: the byte index of the `@` and the text typed
/// after it up to the caret.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mention {
    pub start: usize,
    pub query: String,
}

/// The `@` token the caret sits in, if any. The `@` must open a word (the start
/// of the prompt or follow whitespace), and nothing but non-whitespace may sit
/// between it and the caret. A later `@` belongs to the query (`node@property`). `caret` is a byte index; a caret that is not on a
/// character boundary is not inside a token.
pub fn active(prompt: &str, caret: usize) -> Option<Mention> {
    if caret > prompt.len() || !prompt.is_char_boundary(caret) {
        return None;
    }
    let before = &prompt[..caret];
    // The token is the whole word under the caret: its first character must be
    // the `@` (so `node@property` is one token, not a mention inside a word).
    let at = before.rfind(char::is_whitespace).map_or(0, |i| i + before[i..].chars().next().map_or(1, char::len_utf8));
    let query = before[at..].strip_prefix('@')?;
    Some(Mention { start: at, query: query.to_string() })
}

/// The nodes of a `GET /graph` reply as candidates; `focus` is the selected id.
pub fn node_candidates(graph: &serde_json::Value, focus: Option<&str>) -> Vec<Candidate> {
    graph["graph"]["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|n| {
            let id = n["id"].as_str()?;
            let label = n["label"].as_str().filter(|l| !l.is_empty()).unwrap_or(id);
            Some(Candidate {
                token: id.to_string(),
                label: label.to_string(),
                kind: Kind::Node,
                focused: focus == Some(id),
                recent: 0,
            })
        })
        .collect()
}

/// `node@property` for every property of every node: `params` maps a node kind
/// to its property names (the catalog's). The property of the focused node is
/// focused too.
pub fn property_candidates(
    graph: &serde_json::Value,
    params: &std::collections::HashMap<String, Vec<String>>,
    focus: Option<&str>,
) -> Vec<Candidate> {
    let mut out = Vec::new();
    for n in graph["graph"]["nodes"].as_array().into_iter().flatten() {
        let (Some(id), Some(kind)) = (n["id"].as_str(), n["kind"].as_str()) else { continue };
        let name = n["label"].as_str().filter(|l| !l.is_empty()).unwrap_or(id);
        for prop in params.get(kind).into_iter().flatten() {
            out.push(Candidate {
                token: format!("{id}@{prop}"),
                label: format!("{name}@{prop}"),
                kind: Kind::Property,
                focused: focus == Some(id),
                recent: 0,
            });
        }
    }
    out
}

/// The library's files (rows of `GET /fs`, named by `file`). A name with
/// whitespace cannot sit in a token, so its blanks become `_`.
pub fn file_candidates(files: &[serde_json::Value]) -> Vec<Candidate> {
    files
        .iter()
        .filter_map(|f| f["file"].as_str().or_else(|| f["name"].as_str()))
        .map(|name| Candidate {
            token: name.split_whitespace().collect::<Vec<_>>().join("_"),
            label: name.to_string(),
            kind: Kind::File,
            focused: false,
            recent: 0,
        })
        .collect()
}

/// The overlays grid's cells as `(key, label)`; the key is the token.
pub fn cell_candidates(cells: &[(String, String)]) -> Vec<Candidate> {
    cells
        .iter()
        .map(|(key, label)| Candidate {
            token: key.clone(),
            label: if label.is_empty() { key.clone() } else { label.clone() },
            kind: Kind::Overlay,
            focused: false,
            recent: 0,
        })
        .collect()
}

/// The candidates that match `query`, best first. An empty query matches
/// everything. Duplicate tokens are shown once (the first one wins).
pub fn rank<'a>(query: &str, candidates: &'a [Candidate]) -> Vec<&'a Candidate> {
    let mut seen = std::collections::HashSet::new();
    let mut scored: Vec<(i64, &Candidate)> = candidates
        .iter()
        .filter(|c| seen.insert(c.token.as_str()))
        .filter_map(|c| {
            let fuzzy = if query.is_empty() {
                0
            } else {
                score(query, &c.token).max(score(query, &c.label))?
            };
            let focus = if c.focused { FOCUS_BONUS } else { 0 };
            Some((fuzzy + focus + recency(c.recent), c))
        })
        .collect();
    scored.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| a.1.label.cmp(&b.1.label))
            .then_with(|| a.1.token.cmp(&b.1.token))
    });
    scored.into_iter().map(|(_, c)| c).collect()
}

/// Replaces the `@query` under the caret with `@token ` and returns the new
/// prompt and the caret's byte index after the inserted text.
pub fn splice(prompt: &str, mention: &Mention, caret: usize, token: &str) -> (String, usize) {
    let mut out = String::with_capacity(prompt.len() + token.len() + 2);
    out.push_str(&prompt[..mention.start]);
    out.push('@');
    out.push_str(token);
    out.push(' ');
    let caret_after = out.len();
    out.push_str(&prompt[caret..]);
    (out, caret_after)
}

/// Bonus that puts the focused element first.
const FOCUS_BONUS: i64 = 1_000;
/// Recency's weight: one point per insertion, capped. A match's quality differs
/// by more than the cap, so recency orders ties and never overrides a closer
/// match.
const RECENT_CAP: u32 = 3;

fn recency(recent: u32) -> i64 {
    i64::from(recent.min(RECENT_CAP))
}

/// Subsequence score of `query` in `text`, case-insensitively, or `None` when
/// the query's characters do not all occur in order. A character that starts a
/// word scores most, a run of consecutive matches is rewarded, and a gap is
/// charged, so `gen` ranks `gen-1` above `big-generator`.
fn score(query: &str, text: &str) -> Option<i64> {
    let text: Vec<char> = text.to_lowercase().chars().collect();
    let mut total = 0i64;
    let mut pos = 0usize;
    let mut prev: Option<usize> = None;
    for q in query.to_lowercase().chars() {
        let found = (pos..text.len()).find(|&i| text[i] == q)?;
        total += 10;
        let word_start = found == 0 || !text[found - 1].is_alphanumeric();
        if word_start {
            total += 15;
        }
        match prev {
            Some(p) if found == p + 1 => total += 5,
            Some(p) => total -= (found - p - 1).min(10) as i64,
            None => total -= found.min(10) as i64,
        }
        prev = Some(found);
        pos = found + 1;
    }
    Some(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cand(token: &str, kind: Kind) -> Candidate {
        Candidate { token: token.into(), label: token.into(), kind, focused: false, recent: 0 }
    }

    #[test]
    fn the_token_is_the_at_sign_and_text_up_to_the_caret() {
        let p = "mira @gen";
        assert_eq!(active(p, p.len()), Some(Mention { start: 5, query: "gen".into() }));
        // Caret mid-token: only the text before it counts.
        assert_eq!(active(p, 7), Some(Mention { start: 5, query: "g".into() }));
        assert_eq!(active("@", 1), Some(Mention { start: 0, query: String::new() }));
    }

    #[test]
    fn an_at_inside_a_word_or_a_closed_token_is_not_a_mention() {
        assert_eq!(active("mail@host", 9), None, "@ must open a word");
        assert_eq!(active("@gen after", 10), None, "a space closes the token");
        assert_eq!(active("no sign here", 4), None);
    }

    #[test]
    fn a_caret_off_a_char_boundary_is_not_inside_a_token() {
        let p = "@ñ";
        assert_eq!(active(p, 2), None, "byte 2 is inside the ñ");
        assert_eq!(active(p, p.len()), Some(Mention { start: 0, query: "ñ".into() }));
    }

    #[test]
    fn a_non_matching_candidate_is_out_and_the_rest_keep_their_order() {
        let cands = [cand("gen-1", Kind::Node), cand("fx-2", Kind::Node)];
        let got: Vec<_> = rank("ge", &cands).iter().map(|c| c.token.clone()).collect();
        assert_eq!(got, ["gen-1"]);
    }

    #[test]
    fn a_word_start_match_beats_a_scattered_one() {
        let cands = [cand("big-generator", Kind::Node), cand("gen-1", Kind::Node)];
        let got: Vec<_> = rank("gen", &cands).iter().map(|c| c.token.clone()).collect();
        assert_eq!(got, ["gen-1", "big-generator"]);
    }

    #[test]
    fn the_focused_element_comes_first_whatever_it_matches() {
        let mut focused = cand("zeta", Kind::Node);
        focused.focused = true;
        let cands = [cand("gen-1", Kind::Node), focused];
        // The focused `zeta` does not match "gen" at all, so it is out, not
        // first; with an empty query it leads.
        assert_eq!(rank("gen", &cands).len(), 1);
        let empty: Vec<_> = rank("", &cands).iter().map(|c| c.token.clone()).collect();
        assert_eq!(empty, ["zeta", "gen-1"]);
    }

    #[test]
    fn recency_orders_equal_matches_but_never_beats_a_closer_one() {
        let mut used = cand("gen-2", Kind::Node);
        used.recent = 3;
        let cands = [cand("gen-1", Kind::Node), used];
        let got: Vec<_> = rank("gen", &cands).iter().map(|c| c.token.clone()).collect();
        assert_eq!(got[0], "gen-2", "the used one wins a tie");

        let mut far = cand("big-generator", Kind::Node);
        far.recent = 50;
        let cands = [cand("gen-1", Kind::Node), far];
        let got: Vec<_> = rank("gen", &cands).iter().map(|c| c.token.clone()).collect();
        assert_eq!(got[0], "gen-1", "a close match is not beaten by recency alone");
    }

    #[test]
    fn a_property_token_matches_on_its_node_and_property() {
        let cands = [cand("gen-1@opacity", Kind::Property), cand("fx-9", Kind::Node)];
        let got: Vec<_> = rank("gen@op", &cands).iter().map(|c| c.token.clone()).collect();
        assert_eq!(got, ["gen-1@opacity"]);
    }

    #[test]
    fn a_property_token_is_one_mention_with_the_second_at_in_its_query() {
        let p = "mira @gen-a@sp";
        assert_eq!(active(p, p.len()), Some(Mention { start: 5, query: "gen-a@sp".into() }));
        let m = active(p, p.len()).unwrap();
        assert_eq!(splice(p, &m, p.len(), "gen-a@speed").0, "mira @gen-a@speed ");
    }

    #[test]
    fn every_source_becomes_candidates_of_its_kind() {
        let graph = serde_json::json!({ "graph": { "nodes": [
            { "id": "gen-a", "kind": "gen.turing", "label": "" },
            { "id": "fx-1", "kind": "fx.shake", "label": "Shake" }] } });
        let params = std::collections::HashMap::from([
            ("gen.turing".to_string(), vec!["speed".to_string(), "scale".to_string()]),
            ("fx.shake".to_string(), vec!["amount".to_string()]),
        ]);
        let nodes = node_candidates(&graph, Some("gen-a"));
        assert_eq!(nodes.len(), 2);
        assert!(nodes[0].focused && !nodes[1].focused);
        let props = property_candidates(&graph, &params, Some("gen-a"));
        let tokens: Vec<_> = props.iter().map(|c| c.token.as_str()).collect();
        assert_eq!(tokens, ["gen-a@speed", "gen-a@scale", "fx-1@amount"]);
        assert!(props[0].focused && !props[2].focused);
        assert!(props.iter().all(|c| c.kind == Kind::Property));
        let files = file_candidates(&[serde_json::json!({ "file": "my clip.mp4" })]);
        assert_eq!((files[0].token.as_str(), files[0].kind), ("my_clip.mp4", Kind::File));
        let cells = cell_candidates(&[("rec-1".to_string(), String::new())]);
        assert_eq!((cells[0].label.as_str(), cells[0].kind), ("rec-1", Kind::Overlay));
    }

    /// All four kinds share one ranking: the query picks across them, the
    /// focused node's properties lead an empty query, and `gen-a@sp` finds the
    /// property and not the node.
    #[test]
    fn one_ranking_spans_nodes_files_cells_and_properties() {
        let graph = serde_json::json!({ "graph": { "nodes": [
            { "id": "gen-a", "kind": "gen.turing", "label": "" }] } });
        let params = std::collections::HashMap::from([("gen.turing".to_string(), vec!["speed".to_string()])]);
        let mut all = node_candidates(&graph, Some("gen-a"));
        all.extend(property_candidates(&graph, &params, Some("gen-a")));
        all.extend(file_candidates(&[serde_json::json!({ "file": "kick.wav" })]));
        all.extend(cell_candidates(&[("rec-1".to_string(), "rec".to_string())]));
        let empty: Vec<_> = rank("", &all).iter().map(|c| c.token.clone()).collect();
        assert_eq!(&empty[..2], ["gen-a", "gen-a@speed"], "focus first");
        let got: Vec<_> = rank("gen-a@sp", &all).iter().map(|c| c.token.clone()).collect();
        assert_eq!(got, ["gen-a@speed"]);
        let got: Vec<_> = rank("kick", &all).iter().map(|c| c.token.clone()).collect();
        assert_eq!(got, ["kick.wav"]);
    }

    #[test]
    fn duplicate_tokens_are_listed_once() {
        let cands = [cand("gen-1", Kind::Node), cand("gen-1", Kind::Overlay)];
        assert_eq!(rank("gen", &cands).len(), 1);
    }

    #[test]
    fn splicing_replaces_the_query_and_puts_the_caret_after_the_space() {
        let p = "mira @ge con el";
        let m = active(p, 8).expect("inside the token");
        let (out, caret) = splice(p, &m, 8, "gen-1");
        assert_eq!(out, "mira @gen-1  con el");
        assert_eq!(&out[..caret], "mira @gen-1 ");
    }

    #[test]
    fn splicing_at_the_end_of_the_prompt() {
        let p = "@gen";
        let m = active(p, p.len()).expect("token");
        let (out, caret) = splice(p, &m, p.len(), "gen-1");
        assert_eq!(out, "@gen-1 ");
        assert_eq!(caret, out.len());
    }
}
