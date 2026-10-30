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
/// between it and the caret. `caret` is a byte index; a caret that is not on a
/// character boundary is not inside a token.
pub fn active(prompt: &str, caret: usize) -> Option<Mention> {
    if caret > prompt.len() || !prompt.is_char_boundary(caret) {
        return None;
    }
    let before = &prompt[..caret];
    let at = before.rfind('@')?;
    let query = &before[at + 1..];
    if query.chars().any(char::is_whitespace) {
        return None;
    }
    let opens_word = prompt[..at].chars().last().is_none_or(char::is_whitespace);
    if !opens_word {
        return None;
    }
    Some(Mention { start: at, query: query.to_string() })
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
