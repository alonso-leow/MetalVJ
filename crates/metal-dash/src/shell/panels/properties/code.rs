//! The structure a code field gives its text, the way an editor does: colours
//! by token class (reserved words, types, CONST names, strings, numbers,
//! comments), the indent a new line inherits (one more after an opener), Tab
//! and Shift+Tab as indent and dedent of the selected lines. Pure text in,
//! text out — the egui side only feeds them the caret.

use egui::text::{CCursor, CCursorRange, LayoutJob};
use egui::{Context, Id, Key, Modifiers, TextFormat};

use crate::shell::classes::Theme;

/// The kinds of token the colours tell apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Plain,
    Keyword,
    Type,
    Const,
    Str,
    Number,
    Comment,
}

const PY: &[&str] = &[
    "def", "class", "return", "if", "elif", "else", "for", "while", "in", "not", "and", "or", "import", "from", "as", "with",
    "try", "except", "finally", "raise", "pass", "break", "continue", "lambda", "yield", "global", "None", "True", "False", "is",
];
const JS: &[&str] = &[
    "function", "const", "let", "var", "return", "if", "else", "for", "while", "of", "in", "new", "class", "import", "export",
    "from", "try", "catch", "finally", "throw", "break", "continue", "switch", "case", "default", "typeof", "null", "undefined",
    "true", "false", "this", "async", "await", "yield",
];
const RS: &[&str] = &[
    "fn", "let", "mut", "const", "static", "pub", "struct", "enum", "impl", "trait", "use", "mod", "match", "if", "else", "for",
    "while", "loop", "in", "return", "break", "continue", "as", "ref", "self", "Self", "move", "where", "true", "false", "type",
    "unsafe", "crate", "super", "dyn",
];

fn keywords(lang: &str) -> &'static [&'static str] {
    match lang {
        "js" => JS,
        "rs" => RS,
        _ => PY,
    }
}

/// The text split into `(kind, run)` pieces covering all of it.
pub fn tokens<'a>(text: &'a str, lang: &str) -> Vec<(Kind, &'a str)> {
    let kw = keywords(lang);
    let line_comment: &[&str] = if lang == "py" || lang.is_empty() { &["#"] } else { &["//"] };
    let b = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    let push = |out: &mut Vec<(Kind, &'a str)>, kind, from: usize, to: usize| {
        if to > from {
            out.push((kind, &text[from..to]));
        }
    };
    let mut plain_from = 0;
    while i < b.len() {
        let rest = &text[i..];
        let c = rest.chars().next().unwrap_or(' ');
        let class = if line_comment.iter().any(|m| rest.starts_with(m)) {
            let end = rest.find('\n').map_or(text.len(), |n| i + n);
            Some((Kind::Comment, end))
        } else if c == '"' || c == '\'' {
            let mut j = i + 1;
            while j < b.len() && b[j] as char != c && b[j] != b'\n' {
                j += if b[j] == b'\\' { 2 } else { 1 };
            }
            Some((Kind::Str, (j + 1).min(text.len())))
        } else if c.is_ascii_digit() {
            let mut j = i;
            while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'.' || b[j] == b'_') {
                j += 1;
            }
            Some((Kind::Number, j))
        } else if c.is_alphabetic() || c == '_' {
            let mut j = i;
            for (n, ch) in rest.char_indices() {
                if !(ch.is_alphanumeric() || ch == '_') {
                    break;
                }
                j = i + n + ch.len_utf8();
            }
            let word = &text[i..j];
            let kind = if kw.contains(&word) {
                Kind::Keyword
            } else if word.len() > 1 && word.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_') {
                Kind::Const
            } else if word.chars().next().is_some_and(char::is_uppercase) {
                Kind::Type
            } else {
                Kind::Plain
            };
            (kind != Kind::Plain).then_some((kind, j)).or(Some((Kind::Plain, j)))
        } else {
            None
        };
        match class {
            Some((Kind::Plain, end)) => i = end,
            Some((kind, end)) => {
                push(&mut out, Kind::Plain, plain_from, i);
                push(&mut out, kind, i, end);
                i = end;
                plain_from = end;
            }
            None => i += c.len_utf8(),
        }
    }
    push(&mut out, Kind::Plain, plain_from, text.len());
    out
}

/// The text as one laid-out job, a colour per token class.
pub fn job(theme: &Theme, text: &str, lang: &str, wrap_width: f32) -> LayoutJob {
    let font = theme.font_mono(theme.font_small.size);
    let mut job = LayoutJob::default();
    job.wrap.max_width = wrap_width;
    for (kind, run) in tokens(text, lang) {
        let color = match kind {
            Kind::Plain => theme.ink,
            Kind::Keyword => theme.phosphor_lit,
            Kind::Type => theme.soft,
            Kind::Const => theme.warn,
            Kind::Str => theme.ok,
            Kind::Number => theme.warn,
            Kind::Comment => theme.ink_faint,
        };
        job.append(run, 0.0, TextFormat::simple(font.clone(), color));
    }
    job
}

/// One indent step of a language, in spaces.
pub fn unit(lang: &str) -> usize {
    if lang == "js" { 2 } else { 4 }
}

fn chars_to_byte(text: &str, chars: usize) -> usize {
    text.char_indices().nth(chars).map_or(text.len(), |(b, _)| b)
}

/// The text after Enter at char `at`: the new line keeps the line's indent, and
/// one more step after an opener (`:` for Python, `{ ( [` otherwise). Returns
/// the new text and caret.
pub fn enter(text: &str, at: usize, lang: &str) -> (String, usize) {
    let byte = chars_to_byte(text, at);
    let before = &text[..byte];
    let line = before.rsplit('\n').next().unwrap_or("");
    let indent = line.len() - line.trim_start().len();
    let opener = line.trim_end().chars().last().is_some_and(|c| {
        if lang == "py" || lang.is_empty() { c == ':' } else { matches!(c, '{' | '(' | '[') }
    });
    let pad = indent + if opener { unit(lang) } else { 0 };
    let insert = format!("\n{}", " ".repeat(pad));
    let mut out = String::with_capacity(text.len() + insert.len());
    out.push_str(before);
    out.push_str(&insert);
    out.push_str(&text[byte..]);
    (out, at + insert.chars().count())
}

/// The char range of the lines the selection `from..to` touches, as byte
/// offsets of their starts.
fn line_starts(text: &str, from: usize, to: usize) -> Vec<usize> {
    let (f, t) = (chars_to_byte(text, from), chars_to_byte(text, to));
    let first = text[..f].rfind('\n').map_or(0, |n| n + 1);
    let mut starts = vec![first];
    for (i, c) in text[first..t].char_indices() {
        if c == '\n' && first + i + 1 <= t && first + i + 1 < text.len().max(t) + 1 {
            let s = first + i + 1;
            if s <= t && s != *starts.last().unwrap() {
                starts.push(s);
            }
        }
    }
    starts
}

/// Tab: a caret inserts spaces to the next stop; a selection indents each of
/// its lines. Returns the new text and the new selection.
pub fn indent(text: &str, from: usize, to: usize, lang: &str) -> (String, usize, usize) {
    let step = unit(lang);
    if from == to {
        let byte = chars_to_byte(text, from);
        let col = text[..byte].rsplit('\n').next().map_or(0, |l| l.chars().count());
        let n = step - col % step;
        let mut out = text.to_string();
        out.insert_str(byte, &" ".repeat(n));
        return (out, from + n, from + n);
    }
    let starts = line_starts(text, from, to);
    let mut out = text.to_string();
    for s in starts.iter().rev() {
        out.insert_str(*s, &" ".repeat(step));
    }
    (out, from + step, to + step * starts.len())
}

/// Shift+Tab: removes up to one step of leading spaces from each line the
/// selection touches.
pub fn dedent(text: &str, from: usize, to: usize, lang: &str) -> (String, usize, usize) {
    let step = unit(lang);
    let starts = line_starts(text, from, to);
    let mut out = text.to_string();
    let (mut new_from, mut new_to) = (from, to);
    for (k, s) in starts.iter().enumerate().rev() {
        let spaces = out[*s..].chars().take(step).take_while(|c| *c == ' ').count();
        out.replace_range(*s..*s + spaces, "");
        if k == 0 {
            new_from = from.saturating_sub(spaces);
        }
        new_to = new_to.saturating_sub(spaces);
    }
    (out, new_from, new_to)
}

/// Applies Enter / Tab / Shift+Tab to the focused field `id` before it paints,
/// consuming the key so the field's own handling never sees it.
pub fn handle_keys(ctx: &Context, id: Id, text: &mut String, lang: &str) {
    if !ctx.memory(|m| m.has_focus(id)) {
        return;
    }
    let Some(mut state) = egui::TextEdit::load_state(ctx, id) else { return };
    let Some(range) = state.cursor.char_range() else { return };
    let (p, q) = (range.primary.index.0, range.secondary.index.0);
    let (from, to) = (p.min(q), p.max(q));
    let (next, a, b) = if ctx.input_mut(|i| i.consume_key(Modifiers::SHIFT, Key::Tab)) {
        dedent(text, from, to, lang)
    } else if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Tab)) {
        indent(text, from, to, lang)
    } else if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Enter)) {
        let head = &text[..chars_to_byte(text, from)];
        let tail = &text[chars_to_byte(text, to)..];
        let (t, at) = enter(&format!("{head}{tail}"), from, lang);
        (t, at, at)
    } else {
        return;
    };
    *text = next;
    state.cursor.set_char_range(Some(CCursorRange::two(CCursor::new(a), CCursor::new(b))));
    state.store(ctx, id);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(src: &str, lang: &str) -> Vec<(Kind, String)> {
        tokens(src, lang).into_iter().filter(|(k, _)| *k != Kind::Plain).map(|(k, s)| (k, s.to_string())).collect()
    }

    #[test]
    fn python_reserved_words_strings_numbers_and_comments_are_told_apart() {
        let got = kinds("def run(x):  # go\n    return MAX_ROWS + 1.5 + Vec2(\"a\")", "py");
        assert_eq!(
            got,
            vec![
                (Kind::Keyword, "def".into()),
                (Kind::Comment, "# go".into()),
                (Kind::Keyword, "return".into()),
                (Kind::Const, "MAX_ROWS".into()),
                (Kind::Number, "1.5".into()),
                (Kind::Type, "Vec2".into()),
                (Kind::Str, "\"a\"".into()),
            ]
        );
    }

    #[test]
    fn the_tokens_cover_the_whole_text() {
        let src = "let x = 'a' // c\nfn é() {}";
        for lang in ["js", "rs", "py"] {
            let joined: String = tokens(src, lang).into_iter().map(|(_, s)| s).collect();
            assert_eq!(joined, src);
        }
    }

    #[test]
    fn enter_keeps_the_indent_and_steps_in_after_an_opener() {
        assert_eq!(enter("  x = 1", 7, "py"), ("  x = 1\n  ".to_string(), 10));
        assert_eq!(enter("def f():", 8, "py"), ("def f():\n    ".to_string(), 13));
        assert_eq!(enter("if (a) {", 8, "js"), ("if (a) {\n  ".to_string(), 11));
    }

    #[test]
    fn tab_fills_to_the_next_stop_and_indents_a_selection() {
        assert_eq!(indent("ab", 2, 2, "py"), ("ab  ".to_string(), 4, 4));
        let (t, ..) = indent("a\nb\nc", 0, 3, "py");
        assert_eq!(t, "    a\n    b\nc");
    }

    #[test]
    fn shift_tab_removes_one_step() {
        let (t, a, _) = dedent("    a\n      b", 0, 8, "py");
        assert_eq!(t, "a\n  b");
        assert_eq!(a, 0);
    }
}
