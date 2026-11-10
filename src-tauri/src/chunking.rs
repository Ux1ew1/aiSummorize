//! Разбиение длинного текста на части, не превышающие бюджет токенов.
//! Порядок попыток: абзацы → предложения → слова → символы.

#[derive(Debug, Clone, PartialEq)]
pub struct Chunk {
    pub text: String,
    /// Разделитель, который стоял перед этой частью в оригинале ("" для первой)
    pub sep_before: &'static str,
}

/// Маркер «без разделителя»: часть продолжает то же слово, что и предыдущая.
pub const GLUE: &str = "\u{1}";

struct Piece {
    text: String,
    tokens: usize,
    sep: &'static str,
}

fn sentences(par: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let chars: Vec<char> = par.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        cur.push(c);
        let ends = matches!(c, '.' | '!' | '?' | '…' | '。' | '！' | '？' | '\n');
        let next_ws = chars.get(i + 1).map_or(true, |n| n.is_whitespace());
        let cjk = matches!(c, '。' | '！' | '？');
        if ends && (next_ws || cjk) {
            let t = cur.trim();
            if !t.is_empty() {
                out.push(t.to_string());
            }
            cur.clear();
        }
    }
    let t = cur.trim();
    if !t.is_empty() {
        out.push(t.to_string());
    }
    out
}

/// Режет слишком длинную строку по словам, а слова без пробелов — по символам.
/// Второй элемент пары: true — кусок «склеен» с предыдущим (внутри одного слова), пробел между ними не нужен.
fn hard_split(s: &str, max: usize, count: &dyn Fn(&str) -> usize) -> Vec<(String, bool)> {
    let mut out: Vec<(String, bool)> = Vec::new();
    let mut cur = String::new();
    let mut cur_tokens = 0usize;
    for word in s.split_whitespace() {
        let wt = count(word) + 1;
        if wt > max {
            if !cur.is_empty() {
                out.push((std::mem::take(&mut cur), false));
                cur_tokens = 0;
            }
            let step = (max / 4).max(1);
            let chars: Vec<char> = word.chars().collect();
            for (i, part) in chars.chunks(step).enumerate() {
                out.push((part.iter().collect(), i > 0));
            }
            continue;
        }
        if cur_tokens + wt > max && !cur.is_empty() {
            out.push((std::mem::take(&mut cur), false));
            cur_tokens = 0;
        }
        if !cur.is_empty() {
            cur.push(' ');
        }
        cur.push_str(word);
        cur_tokens += wt;
    }
    if !cur.is_empty() {
        out.push((cur, false));
    }
    out
}

pub fn split_text(text: &str, max_tokens: usize, count: &dyn Fn(&str) -> usize) -> Vec<Chunk> {
    // Небольшой запас: на стыках токенизация может давать чуть больше токенов, чем сумма частей.
    let max = (max_tokens * 96 / 100).max(16);
    let norm = text.replace("\r\n", "\n");

    let mut pieces: Vec<Piece> = Vec::new();
    for par in norm.split("\n\n") {
        let par = par.trim();
        if par.is_empty() {
            continue;
        }
        let par_sep: &'static str = if pieces.is_empty() { "" } else { "\n\n" };
        let pt = count(par);
        if pt <= max {
            pieces.push(Piece { text: par.to_string(), tokens: pt, sep: par_sep });
            continue;
        }
        let mut first = true;
        for sent in sentences(par) {
            let sep = if first { par_sep } else { " " };
            first = false;
            let st = count(&sent);
            if st <= max {
                pieces.push(Piece { text: sent, tokens: st, sep });
            } else {
                let mut inner_first = true;
                for (part, glued) in hard_split(&sent, max, count) {
                    let t = count(&part);
                    let s = if inner_first {
                        sep
                    } else if glued {
                        GLUE
                    } else {
                        " "
                    };
                    inner_first = false;
                    pieces.push(Piece { text: part, tokens: t, sep: s });
                }
            }
        }
    }

    let mut chunks: Vec<Chunk> = Vec::new();
    let mut cur = String::new();
    let mut cur_tokens = 0usize;
    let mut cur_sep: &'static str = "";
    for p in pieces {
        let add = p.tokens + 1;
        if !cur.is_empty() && cur_tokens + add > max {
            chunks.push(Chunk { text: std::mem::take(&mut cur), sep_before: cur_sep });
            cur_tokens = 0;
        }
        if cur.is_empty() {
            cur_sep = if chunks.is_empty() { "" } else { p.sep };
            cur_tokens = p.tokens;
            cur = p.text;
        } else {
            if p.sep != GLUE {
                cur.push_str(if p.sep.is_empty() { " " } else { p.sep });
            }
            cur.push_str(&p.text);
            cur_tokens += add;
        }
    }
    if !cur.is_empty() {
        chunks.push(Chunk { text: cur, sep_before: cur_sep });
    }
    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    fn count(s: &str) -> usize {
        s.chars().count().div_ceil(4)
    }

    #[test]
    fn short_text_is_single_chunk() {
        let c = split_text("Привет, мир.\n\nВторой абзац.", 1000, &count);
        assert_eq!(c.len(), 1);
        assert!(c[0].text.contains("Второй абзац"));
    }

    #[test]
    fn long_text_respects_budget_and_keeps_content() {
        let sentence = "This is a fairly ordinary sentence about nothing in particular. ";
        let text = sentence.repeat(400);
        let chunks = split_text(&text, 200, &count);
        assert!(chunks.len() > 5);
        for c in &chunks {
            assert!(count(&c.text) <= 200, "chunk too large: {}", count(&c.text));
        }
        let words_in: usize = text.split_whitespace().count();
        let words_out: usize = chunks.iter().map(|c| c.text.split_whitespace().count()).sum();
        assert_eq!(words_in, words_out);
    }

    #[test]
    fn huge_word_is_hard_split() {
        let text = "x".repeat(10_000);
        let chunks = split_text(&text, 100, &count);
        assert!(chunks.len() > 1);
        for c in &chunks {
            assert!(count(&c.text) <= 100);
        }
        let total: usize = chunks.iter().map(|c| c.text.chars().count()).sum();
        assert_eq!(total, 10_000);
    }

    #[test]
    fn paragraphs_are_preferred_boundaries() {
        let par = "Слово ".repeat(60);
        let text = format!("{par}\n\n{par}\n\n{par}");
        let chunks = split_text(&text, 100, &count);
        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[1].sep_before, "\n\n");
    }
}
