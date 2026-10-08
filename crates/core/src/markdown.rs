//! Markdown for Quick Look: a small, bounded renderer to Qt's StyledText
//! (DESIGN §6.9).
//!
//! Only a safe subset comes out: headings, paragraphs, lists, task lists,
//! quotes, code blocks, tables, footnotes, bold, italic, strikethrough,
//! inline code and line breaks. StyledText has no table tag, so a table is
//! drawn as aligned columns in a monospace block. Every character of the
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
    let notes = Notes::collect(&lines[..lines.len().min(MAX_LINES)]);
    let inline = |s: &str| inline(s, &notes);

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

    let shown = &lines[..lines.len().min(MAX_LINES)];
    let mut i = 0;
    while i < shown.len() {
        let raw = shown[i];
        i += 1;
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
        // A footnote's text is shown once, at the end.
        if footnote_def(t).is_some() {
            continue;
        }
        // A table: a row of cells, then a |---|:--:| line under it.
        if let Some(aligns) = shown.get(i).and_then(|n| table_rule(n.trim())) {
            if let Some(head) = table_cells(t) {
                flush_para(&mut out, &mut para);
                close_list(&mut out, &mut list);
                let mut rows = vec![head];
                i += 1;
                while let Some(r) = shown.get(i).and_then(|n| table_cells(n.trim())) {
                    rows.push(r);
                    i += 1;
                }
                out.push_str(&table(&rows, &aligns, &notes));
                continue;
            }
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
    if !notes.order.is_empty() {
        out.push_str("<p>――――――――</p>");
        for (n, id) in notes.order.iter().enumerate() {
            let text = notes.text(id).unwrap_or("");
            out.push_str(&format!("<p>[{}] {}</p>", n + 1, inline(text)));
        }
    }
    (out, truncated)
}

/// Footnotes: numbered in the order they are first referred to; a note
/// defined but never referred to is still listed, after the others.
struct Notes {
    order: Vec<String>,
    defs: Vec<(String, String)>,
}

/// At most this many footnotes are numbered.
const MAX_NOTES: usize = 99;

impl Notes {
    fn collect(lines: &[&str]) -> Notes {
        let mut defs = Vec::new();
        let mut order: Vec<String> = Vec::new();
        let mut fence = false;
        for l in lines {
            let t = l.trim();
            if t.starts_with("```") || t.starts_with("~~~") {
                fence = !fence;
                continue;
            }
            if fence {
                continue;
            }
            let body = match footnote_def(t) {
                Some((id, text)) => {
                    defs.push((id.to_string(), text.to_string()));
                    text
                }
                None => t,
            };
            let mut rest = body;
            while let Some(p) = rest.find("[^") {
                let after = &rest[p + 2..];
                match after.find(']') {
                    Some(e) if e > 0 && !after[..e].contains(['[', ' ']) => {
                        let id = &after[..e];
                        if !order.iter().any(|o| o == id) && order.len() < MAX_NOTES {
                            order.push(id.to_string());
                        }
                        rest = &after[e + 1..];
                    }
                    _ => rest = after,
                }
            }
        }
        for (id, _) in &defs {
            if !order.iter().any(|o| o == id) && order.len() < MAX_NOTES {
                order.push(id.clone());
            }
        }
        // A reference with no text is left as written.
        order.retain(|id| defs.iter().any(|(d, _)| d == id));
        Notes { order, defs }
    }
    fn number(&self, id: &str) -> Option<usize> {
        self.order.iter().position(|o| o == id).map(|n| n + 1)
    }
    fn text(&self, id: &str) -> Option<&str> {
        self.defs
            .iter()
            .find(|(d, _)| d == id)
            .map(|(_, t)| t.as_str())
    }
}

/// `[^id]: text`
fn footnote_def(t: &str) -> Option<(&str, &str)> {
    let r = t.strip_prefix("[^")?;
    let e = r.find("]:")?;
    let id = &r[..e];
    if id.is_empty() || id.contains([' ', '[', ']']) {
        return None;
    }
    Some((id, r[e + 2..].trim()))
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Align {
    Left,
    Centre,
    Right,
}

/// The `|---|:--:|---:|` line under a table's header, as alignments.
fn table_rule(t: &str) -> Option<Vec<Align>> {
    let cells = table_cells(t)?;
    let mut out = Vec::new();
    for c in cells {
        let c = c.trim();
        let core = c.trim_start_matches(':').trim_end_matches(':');
        if core.is_empty() || !core.chars().all(|x| x == '-') {
            return None;
        }
        out.push(match (c.starts_with(':'), c.ends_with(':')) {
            (true, true) => Align::Centre,
            (false, true) => Align::Right,
            _ => Align::Left,
        });
    }
    Some(out)
}

/// The cells of a `| a | b |` row (outer pipes optional, `\|` is a pipe).
fn table_cells(t: &str) -> Option<Vec<String>> {
    if !t.contains('|') {
        return None;
    }
    let body = t.strip_prefix('|').unwrap_or(t);
    let body = if body.ends_with('|') && !body.ends_with("\\|") {
        &body[..body.len() - 1]
    } else {
        body
    };
    let mut cells = vec![String::new()];
    let mut esc = false;
    for ch in body.chars() {
        if esc {
            if ch != '|' {
                cells.last_mut().unwrap().push('\\');
            }
            cells.last_mut().unwrap().push(ch);
            esc = false;
        } else if ch == '\\' {
            esc = true;
        } else if ch == '|' {
            cells.push(String::new());
        } else {
            cells.last_mut().unwrap().push(ch);
        }
    }
    Some(cells.into_iter().map(|c| c.trim().to_string()).collect())
}

/// At most this many columns, and this many characters in one cell.
const MAX_COLUMNS: usize = 12;
const MAX_CELL: usize = 40;

/// A table as aligned columns in a monospace block, header in bold.
fn table(rows: &[Vec<String>], aligns: &[Align], notes: &Notes) -> String {
    let cols = rows
        .iter()
        .map(Vec::len)
        .max()
        .unwrap_or(0)
        .min(MAX_COLUMNS);
    let cell = |r: &Vec<String>, c: usize| -> Vec<char> {
        let p = plain(r.get(c).map(String::as_str).unwrap_or(""), notes);
        let mut v: Vec<char> = p.chars().collect();
        if v.len() > MAX_CELL {
            v.truncate(MAX_CELL - 1);
            v.push('…');
        }
        v
    };
    let widths: Vec<usize> = (0..cols)
        .map(|c| rows.iter().map(|r| cell(r, c).len()).max().unwrap_or(0))
        .collect();
    let mut out = String::from("<pre>");
    for (ri, r) in rows.iter().enumerate() {
        let mut line = String::new();
        for (c, width) in widths.iter().enumerate() {
            let v = cell(r, c);
            let gap = width - v.len();
            let (l, rt) = match aligns.get(c).copied().unwrap_or(Align::Left) {
                Align::Left => (0, gap),
                Align::Right => (gap, 0),
                Align::Centre => (gap / 2, gap - gap / 2),
            };
            if c > 0 {
                line.push_str(" │ ");
            }
            line.push_str(&" ".repeat(l));
            line.push_str(&escape(&v.iter().collect::<String>()));
            line.push_str(&" ".repeat(rt));
        }
        let line = line.trim_end().to_string();
        if ri == 0 {
            out.push_str(&format!("<b>{line}</b><br>"));
            let rule: Vec<String> = widths.iter().map(|w| "─".repeat(*w)).collect();
            out.push_str(&rule.join("─┼─"));
            out.push_str("<br>");
        } else {
            out.push_str(&line);
            out.push_str("<br>");
        }
    }
    out.push_str("</pre>");
    out
}

/// A cell's text as it reads, without its markdown (for column widths).
fn plain(s: &str, notes: &Notes) -> String {
    let c: Vec<char> = s.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < c.len() {
        if c[i] == '!' && c.get(i + 1) == Some(&'[') {
            if let Some((text, next)) = link_at(&c, i + 1) {
                out.push_str(&format!("[image: {text}]"));
                i = next;
                continue;
            }
        }
        if c[i] == '[' {
            if let Some((n, next)) = note_at(&c, i, notes) {
                out.push_str(&format!("[{n}]"));
                i = next;
                continue;
            }
            if let Some((text, next)) = link_at(&c, i) {
                out.push_str(&text);
                i = next;
                continue;
            }
        }
        if c[i] == '`' || ((c[i] == '*' || c[i] == '~') && c.get(i + 1) == Some(&c[i])) {
            i += if c[i] == '`' { 1 } else { 2 };
            continue;
        }
        if c[i] == '\\' && i + 1 < c.len() && c[i + 1].is_ascii_punctuation() {
            out.push(c[i + 1]);
            i += 2;
            continue;
        }
        out.push(c[i]);
        i += 1;
    }
    out
}

/// `[^id]` at `c[i] == '['`: the note's number and the index after it.
fn note_at(c: &[char], i: usize, notes: &Notes) -> Option<(usize, usize)> {
    if c.get(i + 1) != Some(&'^') {
        return None;
    }
    let close = i + 2 + c[i + 2..].iter().position(|x| *x == ']')?;
    let id: String = c[i + 2..close].iter().collect();
    Some((notes.number(&id)?, close + 1))
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
fn inline(s: &str, notes: &Notes) -> String {
    let mut out = String::new();
    let c: Vec<char> = s.chars().collect();
    let mut i = 0;
    let (mut bold, mut ital, mut strike) = (false, false, false);
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
        // [^note] → [1]
        if c[i] == '[' {
            if let Some((n, next)) = note_at(&c, i, notes) {
                out.push_str(&format!("<font color=\"{CODE_COLOUR}\">[{n}]</font>"));
                i = next;
                continue;
            }
        }
        // ~~gone~~ → struck through.
        if c[i] == '~' && c.get(i + 1) == Some(&'~') {
            out.push_str(if strike { "</s>" } else { "<s>" });
            strike = !strike;
            i += 2;
            continue;
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
    if strike {
        out.push_str("</s>");
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
    fn tables_line_up_and_keep_their_alignment() {
        let src = "| Name | Size | Kind |\n|:-----|-----:|:----:|\n| a.txt | 1 KB | text |\n| <b>photo</b>.jpg | 120 KB | **image** |\nafter";
        let (h, _) = to_styled(src);
        assert!(
            h.contains("<pre><b>Name             │   Size │ Kind</b><br>"),
            "{h}"
        );
        assert!(h.contains("─────────────────┼────────┼──────<br>"), "{h}");
        assert!(h.contains("a.txt            │   1 KB │ text<br>"), "{h}");
        // Markup in a cell is shown, never obeyed; markdown is read.
        assert!(
            h.contains("&lt;b&gt;photo&lt;/b&gt;.jpg │ 120 KB │ image<br>"),
            "{h}"
        );
        assert!(h.contains("</pre><p>after</p>"), "{h}");
    }

    #[test]
    fn a_pipe_in_prose_is_not_a_table() {
        let (h, _) = to_styled("this | that\nand more");
        assert!(!h.contains("<pre>"), "{h}");
        let (h, _) = to_styled("| a | b |\n| x | y |");
        assert!(!h.contains("<pre>"), "{h}");
    }

    #[test]
    fn wide_tables_are_bounded() {
        let long = "x".repeat(200);
        let src = format!("| {long} |\n|---|\n| y |");
        let (h, _) = to_styled(&src);
        assert!(h.contains(&format!("{}…", "x".repeat(MAX_CELL - 1))), "{h}");
        assert!(!h.contains(&"x".repeat(MAX_CELL + 1)), "{h}");
    }

    #[test]
    fn footnotes_are_numbered_and_listed_at_the_end() {
        let src = "Fast[^speed] and safe[^s].\n\n[^s]: Nothing is overwritten.\n[^speed]: 100k files.\n\nMore.";
        let (h, _) = to_styled(src);
        assert!(h.contains(&format!("Fast<font color=\"{CODE_COLOUR}\">[1]</font> and safe<font color=\"{CODE_COLOUR}\">[2]</font>.")), "{h}");
        assert!(
            h.ends_with("<p>――――――――</p><p>[1] 100k files.</p><p>[2] Nothing is overwritten.</p>"),
            "{h}"
        );
        assert!(!h.contains("[^"), "{h}");
        // A reference with no note stays as written.
        let (h, _) = to_styled("see[^none]");
        assert!(h.contains("see[^none]"), "{h}");
    }

    #[test]
    fn strikethrough() {
        let (h, _) = to_styled("~~old~~ new ~~open");
        assert!(h.contains("<s>old</s> new <s>open</s>"), "{h}");
    }

    #[test]
    fn the_colour_marker_cannot_be_forged() {
        let (h, _) = to_styled("a\u{1}b");
        assert!(!h.contains('\u{1}'), "{h:?}");
    }
}
