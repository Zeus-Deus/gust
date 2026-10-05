//! Markdown for Quick Look: a small, bounded renderer to Qt's StyledText
//! (DESIGN §6.9).
//!
//! Only a safe subset comes out: headings, paragraphs, lists, quotes, code
//! blocks, bold, italic, inline code and line breaks. Every character of the
//! file is HTML-escaped first, so a file cannot inject markup of its own.
//! Images are never loaded (a remote image is a tracking pixel): they become
//! `[image: alt]`. Links show their text only and are not clickable, so a
//! preview can never send anyone anywhere.
//!
//! Inline code is wrapped in `<font color="\u{1}">`; the view swaps `\u{1}`
//! for a theme colour, so no colour is decided here.

/// The marker the view replaces with the theme's code colour.
pub const CODE_COLOUR: char = '\u{1}';

/// At most this many source lines are rendered.
pub const MAX_LINES: usize = 400;

/// Render `src` (already-sanitised lines are not required: this escapes).
/// Returns the StyledText and whether the source was cut short.
pub fn to_styled(src: &str) -> (String, bool) {
    let mut out = String::new();
    let mut in_code = false;
    let mut para: Vec<String> = Vec::new();
    let mut list: Option<&'static str> = None;
    let lines: Vec<&str> = src.lines().collect();
    let truncated = lines.len() > MAX_LINES;

    let flush_para = |out: &mut String, para: &mut Vec<String>| {
        if !para.is_empty() {
            out.push_str("<p>");
            out.push_str(&inline(&para.join(" ")));
            out.push_str("</p>");
            para.clear();
        }
    };
    let close_list = |out: &mut String, list: &mut Option<&'static str>| {
        if let Some(tag) = list.take() {
            out.push_str(&format!("</{tag}>"));
        }
    };

    for raw in lines.iter().take(MAX_LINES) {
        let line = raw.trim_end();
        let t = line.trim_start();
        if t.starts_with("```") || t.starts_with("~~~") {
            flush_para(&mut out, &mut para);
            close_list(&mut out, &mut list);
            out.push_str(if in_code { "</pre>" } else { "<pre>" });
            in_code = !in_code;
            continue;
        }
        if in_code {
            out.push_str(&escape(line));
            out.push_str("<br>");
            continue;
        }
        if t.is_empty() {
            flush_para(&mut out, &mut para);
            close_list(&mut out, &mut list);
            continue;
        }
        if let Some((level, text)) = heading(t) {
            flush_para(&mut out, &mut para);
            close_list(&mut out, &mut list);
            out.push_str(&format!("<h{level}>{}</h{level}>", inline(text)));
            continue;
        }
        if is_rule(t) {
            flush_para(&mut out, &mut para);
            close_list(&mut out, &mut list);
            out.push_str("<p>――――――――</p>");
            continue;
        }
        if let Some((tag, item)) = list_item(t) {
            flush_para(&mut out, &mut para);
            if list != Some(tag) {
                close_list(&mut out, &mut list);
                out.push_str(&format!("<{tag}>"));
                list = Some(tag);
            }
            out.push_str(&format!("<li>{}</li>", inline(&item)));
            continue;
        }
        if let Some(q) = t.strip_prefix('>') {
            flush_para(&mut out, &mut para);
            close_list(&mut out, &mut list);
            out.push_str(&format!("<p><i>│ {}</i></p>", inline(q.trim_start())));
            continue;
        }
        // A list item's continuation line stays in the item's paragraph.
        close_list(&mut out, &mut list);
        para.push(t.to_string());
    }
    flush_para(&mut out, &mut para);
    close_list(&mut out, &mut list);
    if in_code {
        out.push_str("</pre>");
    }
    (out, truncated)
}

fn heading(t: &str) -> Option<(usize, &str)> {
    let n = t.bytes().take_while(|b| *b == b'#').count();
    if (1..=6).contains(&n) && t[n..].starts_with(' ') {
        // h1 is huge in StyledText; the panel is small. Start at h2.
        Some((
            (n + 1).min(6),
            t[n..].trim().trim_end_matches('#').trim_end(),
        ))
    } else {
        None
    }
}

fn is_rule(t: &str) -> bool {
    let c: String = t.chars().filter(|c| !c.is_whitespace()).collect();
    c.len() >= 3
        && (c.chars().all(|x| x == '-')
            || c.chars().all(|x| x == '*')
            || c.chars().all(|x| x == '_'))
}

fn list_item(t: &str) -> Option<(&'static str, String)> {
    for m in ["- ", "* ", "+ "] {
        if let Some(rest) = t.strip_prefix(m) {
            // Task lists read as boxes.
            let item = if let Some(r) = rest.strip_prefix("[ ] ") {
                format!("☐ {r}")
            } else if let Some(r) = rest
                .strip_prefix("[x] ")
                .or_else(|| rest.strip_prefix("[X] "))
            {
                format!("☑ {r}")
            } else {
                rest.to_string()
            };
            return Some(("ul", item));
        }
    }
    let digits = t.bytes().take_while(u8::is_ascii_digit).count();
    if digits > 0 && digits <= 9 {
        let rest = &t[digits..];
        if let Some(r) = rest.strip_prefix(". ").or_else(|| rest.strip_prefix(") ")) {
            return Some(("ol", r.to_string()));
        }
    }
    None
}

/// HTML-escape everything, then add bold, italic, code, links and images.
fn inline(s: &str) -> String {
    let mut out = String::new();
    let c: Vec<char> = s.chars().collect();
    let mut i = 0;
    let (mut bold, mut ital) = (false, false);
    while i < c.len() {
        // Inline code: copied verbatim (escaped), no emphasis inside.
        if c[i] == '`' {
            if let Some(end) = c[i + 1..].iter().position(|x| *x == '`') {
                let code: String = c[i + 1..i + 1 + end].iter().collect();
                out.push_str(&format!(
                    "<font color=\"{CODE_COLOUR}\">{}</font>",
                    escape(&code)
                ));
                i += end + 2;
                continue;
            }
        }
        // ![alt](url) → [image: alt]; never loaded.
        if c[i] == '!' && c.get(i + 1) == Some(&'[') {
            if let Some((text, next)) = link_at(&c, i + 1) {
                out.push_str(&format!("[image: {}]", escape(&text)));
                i = next;
                continue;
            }
        }
        // [text](url) → text, underlined, not clickable.
        if c[i] == '[' {
            if let Some((text, next)) = link_at(&c, i) {
                out.push_str(&format!("<u>{}</u>", escape(&text)));
                i = next;
                continue;
            }
        }
        if (c[i] == '*' || c[i] == '_') && c.get(i + 1) == Some(&c[i]) {
            out.push_str(if bold { "</b>" } else { "<b>" });
            bold = !bold;
            i += 2;
            continue;
        }
        if c[i] == '*'
            || (c[i] == '_' && (i == 0 || !c[i - 1].is_alphanumeric()))
            || (c[i] == '_' && ital)
        {
            out.push_str(if ital { "</i>" } else { "<i>" });
            ital = !ital;
            i += 1;
            continue;
        }
        if c[i] == '\\' && i + 1 < c.len() && c[i + 1].is_ascii_punctuation() {
            out.push_str(&escape(&c[i + 1].to_string()));
            i += 2;
            continue;
        }
        out.push_str(&escape(&c[i].to_string()));
        i += 1;
    }
    if ital {
        out.push_str("</i>");
    }
    if bold {
        out.push_str("</b>");
    }
    out
}

/// `[text](url)` starting at `c[i] == '['`: the text and the index after it.
fn link_at(c: &[char], i: usize) -> Option<(String, usize)> {
    let close = i + 1 + c[i + 1..].iter().position(|x| *x == ']')?;
    if c.get(close + 1) != Some(&'(') {
        return None;
    }
    let end = close + 2 + c[close + 2..].iter().position(|x| *x == ')')?;
    Some((c[i + 1..close].iter().collect(), end + 1))
}

/// Escape for StyledText. Control characters become a visible dot, as in
/// the plain-text peek.
pub fn escape(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            '\t' => o.push_str("    "),
            c if c.is_control() || c == CODE_COLOUR => o.push('·'),
            c => o.push(c),
        }
    }
    o
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markup_in_the_file_is_shown_not_obeyed() {
        let (h, _) = to_styled("<img src=\"http://x/pixel.png\"> <b>hi</b> & <font color=red>");
        assert!(!h.contains("<img"), "{h}");
        assert!(
            h.contains("&lt;img src=&quot;http://x/pixel.png&quot;&gt;"),
            "{h}"
        );
        assert!(h.contains("&lt;b&gt;hi&lt;/b&gt; &amp; &lt;font"), "{h}");
    }

    #[test]
    fn images_are_never_loaded_and_links_go_nowhere() {
        let (h, _) = to_styled("![logo](https://tracker.example/p.gif) see [docs](https://x.y)");
        assert!(h.contains("[image: logo]"), "{h}");
        assert!(h.contains("<u>docs</u>"), "{h}");
        assert!(!h.contains("tracker"), "{h}");
        assert!(!h.contains("href"), "{h}");
    }

    #[test]
    fn headings_lists_code_and_emphasis() {
        let src = "# Title\n\nSome **bold** and *it* and `x<y`.\n\n- one\n- [x] done\n\n1. first\n2. second\n\n```\nfn main() {}\n```\n> quote";
        let (h, t) = to_styled(src);
        assert!(!t);
        assert!(h.contains("<h2>Title</h2>"), "{h}");
        assert!(h.contains("<b>bold</b>"), "{h}");
        assert!(h.contains("<i>it</i>"), "{h}");
        assert!(
            h.contains(&format!("<font color=\"{CODE_COLOUR}\">x&lt;y</font>")),
            "{h}"
        );
        assert!(h.contains("<ul><li>one</li><li>☑ done</li></ul>"), "{h}");
        assert!(h.contains("<ol><li>first</li><li>second</li></ol>"), "{h}");
        assert!(h.contains("<pre>fn main() {}<br></pre>"), "{h}");
        assert!(h.contains("│ quote"), "{h}");
    }

    #[test]
    fn snake_case_words_stay_words() {
        let (h, _) = to_styled("call check_wake_sources now");
        assert!(h.contains("check_wake_sources"), "{h}");
        assert!(!h.contains("<i>"), "{h}");
    }

    #[test]
    fn long_files_are_bounded_and_unclosed_blocks_close() {
        let big = "line\n".repeat(MAX_LINES + 10);
        assert!(to_styled(&big).1);
        let (h, _) = to_styled("```\nnever closed");
        assert!(h.ends_with("</pre>"), "{h}");
        let (h, _) = to_styled("**never closed");
        assert!(h.ends_with("</b></p>"), "{h}");
    }

    #[test]
    fn the_colour_marker_cannot_be_forged() {
        let (h, _) = to_styled("a\u{1}b");
        assert!(!h.contains('\u{1}'), "{h:?}");
    }
}
