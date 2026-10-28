//! Discord-flavoured markdown → a flat block/span structure that's easy to lay out in an immediate-mode UI.

use unicode_segmentation::UnicodeSegmentation;

use crate::Snowflake;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Style {
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
    /// Index of the enclosing spoiler within the message, so each can be revealed separately.
    pub spoiler: Option<u16>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Span {
    Text(String, Style),
    Code(String),
    /// `title` is the hover text a masked link may carry: `[text](url "title")`.
    Link { text: String, url: String, style: Style, title: Option<String> },
    User(Snowflake),
    Role(Snowflake),
    Channel(Snowflake),
    /// A custom server emoji.
    Emoji { id: Snowflake, name: String, animated: bool },
    /// A Unicode emoji (one grapheme cluster), drawn as an image.
    UnicodeEmoji(String),
    /// `<t:unix:style>`; the style character defaults to 'f'.
    Timestamp(i64, char),
    /// @everyone or @here.
    Everyone(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Block {
    Paragraph(Vec<Span>),
    Heading(u8, Vec<Span>),
    Subtext(Vec<Span>),
    ListItem { depth: u8, spans: Vec<Span> },
    Quote(Vec<Block>),
    Code { lang: Option<String>, code: String },
}

pub fn parse(text: &str) -> Vec<Block> {
    let mut spoilers = 0u16;
    let mut out = Vec::new();
    // Code blocks come first: nothing inside them is markdown.
    for (is_code, seg) in split_code_blocks(text) {
        if is_code {
            out.push(code_block(seg));
        } else {
            parse_lines(seg, &mut out, &mut spoilers);
        }
    }
    out
}

/// Whether a message is only emoji (and whitespace), which Discord shows large.
pub fn is_emoji_only(blocks: &[Block]) -> bool {
    let [Block::Paragraph(spans)] = blocks else { return false };
    let mut count = 0;
    for s in spans {
        match s {
            Span::Emoji { .. } | Span::UnicodeEmoji(_) => count += 1,
            Span::Text(t, _) if t.trim().is_empty() => {}
            _ => return false,
        }
    }
    (1..=30).contains(&count)
}

fn split_code_blocks(text: &str) -> Vec<(bool, &str)> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("```") {
        let after = &rest[start + 3..];
        let Some(end) = after.find("```") else { break };
        if start > 0 {
            out.push((false, &rest[..start]));
        }
        out.push((true, &after[..end]));
        rest = &after[end + 3..];
        // A newline right after the closing fence belongs to the block.
        rest = rest.strip_prefix('\n').unwrap_or(rest);
    }
    if !rest.is_empty() {
        out.push((false, rest));
    }
    out
}

fn code_block(inner: &str) -> Block {
    // ```lang\ncode``` — a language tag only counts if it's alone on the first line.
    if let Some((first, body)) = inner.split_once('\n') {
        let tag = first.trim();
        if !tag.is_empty() && !tag.contains(' ') && tag.len() <= 20 {
            return Block::Code { lang: Some(tag.to_owned()), code: body.trim_end_matches('\n').to_owned() };
        }
        if tag.is_empty() {
            return Block::Code { lang: None, code: body.trim_end_matches('\n').to_owned() };
        }
    }
    Block::Code { lang: None, code: inner.trim_matches('\n').to_owned() }
}

fn parse_lines(text: &str, out: &mut Vec<Block>, spoilers: &mut u16) {
    let mut para = String::new();
    let mut quote: Option<String> = None;
    let flush_para = |para: &mut String, out: &mut Vec<Block>, spoilers: &mut u16| {
        let text = para.trim_end_matches('\n');
        if !text.is_empty() {
            out.push(Block::Paragraph(parse_inline(text, spoilers)));
        }
        para.clear();
    };

    let mut lines = text.split('\n').peekable();
    while let Some(line) = lines.next() {
        // ">>> " quotes the rest of the message.
        if let Some(rest) = line.strip_prefix(">>> ") {
            flush_para(&mut para, out, spoilers);
            let mut body = rest.to_owned();
            for l in lines.by_ref() {
                body.push('\n');
                body.push_str(l);
            }
            let mut inner = Vec::new();
            parse_lines(&body, &mut inner, spoilers);
            out.push(Block::Quote(inner));
            break;
        }
        if let Some(rest) = line.strip_prefix("> ").or(if line == ">" { Some("") } else { None }) {
            flush_para(&mut para, out, spoilers);
            let q = quote.get_or_insert_with(String::new);
            if !q.is_empty() {
                q.push('\n');
            }
            q.push_str(rest);
            if !lines.peek().is_some_and(|l| l.starts_with("> ") || *l == ">") {
                let mut inner = Vec::new();
                parse_lines(&quote.take().unwrap(), &mut inner, spoilers);
                out.push(Block::Quote(inner));
            }
            continue;
        }
        let heading = ["### ", "## ", "# "].iter().enumerate().find_map(|(i, p)| line.strip_prefix(p).map(|r| (3 - i as u8, r)));
        if let Some((level, rest)) = heading.filter(|(_, r)| !r.trim().is_empty()) {
            flush_para(&mut para, out, spoilers);
            out.push(Block::Heading(level, parse_inline(rest, spoilers)));
            continue;
        }
        if let Some(rest) = line.strip_prefix("-# ").filter(|r| !r.trim().is_empty()) {
            flush_para(&mut para, out, spoilers);
            out.push(Block::Subtext(parse_inline(rest, spoilers)));
            continue;
        }
        let trimmed = line.trim_start_matches(' ');
        let indent = line.len() - trimmed.len();
        if let Some(rest) = trimmed.strip_prefix("- ").or_else(|| trimmed.strip_prefix("* ")) {
            flush_para(&mut para, out, spoilers);
            out.push(Block::ListItem { depth: (indent / 2).min(4) as u8, spans: parse_inline(rest, spoilers) });
            continue;
        }
        para.push_str(line);
        para.push('\n');
    }
    flush_para(&mut para, out, spoilers);
}

pub fn parse_inline(text: &str, spoilers: &mut u16) -> Vec<Span> {
    let mut out = Vec::new();
    inline(text, Style::default(), &mut out, spoilers);
    merge_text(out)
}

fn merge_text(spans: Vec<Span>) -> Vec<Span> {
    let mut out: Vec<Span> = Vec::with_capacity(spans.len());
    for s in spans {
        if let (Some(Span::Text(prev, ps)), Span::Text(t, st)) = (out.last_mut(), &s) {
            if ps == st {
                prev.push_str(t);
                continue;
            }
        }
        out.push(s);
    }
    out
}

/// Finds an unescaped closing delimiter with non-empty content.
fn find_close(s: &str, delim: &str) -> Option<usize> {
    let mut from = 0;
    while let Some(i) = s[from..].find(delim) {
        let at = from + i;
        let escaped = s[..at].ends_with('\\');
        if at > 0 && !escaped {
            // In a longer run of the same character ("c***"), the delimiter is the end of the run, so inner
            // delimiters close first.
            let ch = delim.as_bytes()[0];
            if delim.bytes().all(|b| b == ch) {
                let run = s[at..].bytes().take_while(|&b| b == ch).count();
                return Some(at + run - delim.len());
            }
            return Some(at);
        }
        from = at + delim.len().max(1);
        if from >= s.len() {
            break;
        }
    }
    None
}

fn push_text(out: &mut Vec<Span>, text: &str, style: Style) {
    if text.is_empty() {
        return;
    }
    // Split emoji out of plain text so they can be drawn as images.
    let mut buf = String::new();
    for g in text.graphemes(true) {
        if is_emoji_grapheme(g) {
            if !buf.is_empty() {
                out.push(Span::Text(std::mem::take(&mut buf), style));
            }
            out.push(Span::UnicodeEmoji(g.to_owned()));
        } else {
            buf.push_str(g);
        }
    }
    if !buf.is_empty() {
        out.push(Span::Text(buf, style));
    }
}

fn inline(s: &str, style: Style, out: &mut Vec<Span>, spoilers: &mut u16) {
    let mut text_start = 0;
    let mut i = 0;
    let bytes = s.as_bytes();

    macro_rules! flush {
        () => {
            push_text(out, &s[text_start..i], style);
        };
    }

    while i < s.len() {
        let rest = &s[i..];
        let c = bytes[i];

        // Escapes.
        if c == b'\\' {
            if let Some(next) = rest[1..].chars().next().filter(|c| c.is_ascii_punctuation()) {
                flush!();
                push_text(out, &next.to_string(), style);
                i += 1 + next.len_utf8();
                text_start = i;
                continue;
            }
        }

        // Inline code.
        if c == b'`' {
            let ticks = if rest.starts_with("``") { "``" } else { "`" };
            if let Some(end) = rest[ticks.len()..].find(ticks).filter(|&e| e > 0) {
                flush!();
                let code = &rest[ticks.len()..ticks.len() + end];
                out.push(Span::Code(code.trim().to_owned()));
                i += ticks.len() * 2 + end;
                text_start = i;
                continue;
            }
        }

        // Paired delimiters, longest first.
        let pairs: [(&str, fn(&mut Style, &mut u16)); 6] = [
            ("***", |st, _| {
                st.bold = true;
                st.italic = true;
            }),
            ("**", |st, _| st.bold = true),
            ("__", |st, _| st.underline = true),
            ("~~", |st, _| st.strike = true),
            ("||", |st, n| {
                st.spoiler = Some(*n);
                *n += 1;
            }),
            ("*", |st, _| st.italic = true),
        ];
        let mut matched = false;
        for (delim, apply) in pairs {
            if !rest.starts_with(delim) {
                continue;
            }
            let body = &rest[delim.len()..];
            // A single '*' must hug its content, so "2 * 3 * 4" stays literal.
            if delim == "*" && body.starts_with(' ') {
                break;
            }
            if let Some(end) = find_close(body, delim) {
                if delim == "*" && body[..end].ends_with(' ') {
                    break;
                }
                flush!();
                let mut st = style;
                apply(&mut st, spoilers);
                inline(&body[..end], st, out, spoilers);
                i += delim.len() * 2 + end;
                text_start = i;
                matched = true;
            }
            break;
        }
        if matched {
            continue;
        }

        // _italic_ only at word boundaries, so snake_case_names stay intact.
        if c == b'_' && !rest.starts_with("__") {
            let boundary_before = s[..i].chars().last().is_none_or(|p| !p.is_alphanumeric());
            if boundary_before {
                if let Some(end) = find_close(&rest[1..], "_") {
                    let after = rest[1 + end + 1..].chars().next();
                    if after.is_none_or(|a| !a.is_alphanumeric()) && !rest[1..].starts_with(' ') {
                        flush!();
                        let mut st = style;
                        st.italic = true;
                        inline(&rest[1..1 + end], st, out, spoilers);
                        i += end + 2;
                        text_start = i;
                        continue;
                    }
                }
            }
        }

        // <@id> <@!id> <@&id> <#id> <:name:id> <a:name:id> <t:123:R> <https://…>
        if c == b'<' {
            if let Some(end) = rest.find('>') {
                let inner = &rest[1..end];
                if let Some(span) = angle_span(inner) {
                    flush!();
                    out.push(match span {
                        Span::Link { text, url, title, .. } => Span::Link { text, url, style, title },
                        other => other,
                    });
                    i += end + 1;
                    text_start = i;
                    continue;
                }
            }
        }

        // [text](https://…) or [text](https://… "title")
        if c == b'[' {
            if let Some(close) = rest.find("](") {
                let label = &rest[1..close];
                if let Some((url, title, used)) = masked_target(&rest[close + 2..]) {
                    if !label.is_empty() && !label.contains('\n') && is_url(url) {
                        flush!();
                        let (text, style) = link_label(label, style);
                        out.push(Span::Link { text: text.to_owned(), url: url.to_owned(), style, title: title.map(str::to_owned) });
                        i += close + 2 + used;
                        text_start = i;
                        continue;
                    }
                }
            }
        }

        // Bare links.
        if (c == b'h') && (rest.starts_with("https://") || rest.starts_with("http://")) {
            let mut end = rest.find(|ch: char| ch.is_whitespace() || ch == '<').unwrap_or(rest.len());
            // Trailing punctuation usually belongs to the sentence, not the URL.
            while end > 0 && rest[..end].ends_with(['.', ',', ':', ';', '"', '\'', '!', '?']) {
                end -= 1;
            }
            if rest[..end].ends_with(')') && !rest[..end].contains('(') {
                end -= 1;
            }
            let url = &rest[..end];
            if url.len() > "https://".len() {
                flush!();
                out.push(Span::Link { text: url.to_owned(), url: url.to_owned(), style, title: None });
                i += end;
                text_start = i;
                continue;
            }
        }

        if c == b'@' {
            for word in ["@everyone", "@here"] {
                if rest.starts_with(word) {
                    flush!();
                    out.push(Span::Everyone(word.to_owned()));
                    i += word.len();
                    text_start = i;
                    matched = true;
                    break;
                }
            }
            if matched {
                continue;
            }
        }

        i += rest.chars().next().map_or(1, char::len_utf8);
    }
    push_text(out, &s[text_start..], style);
}

/// What follows a masked link's `](`: the URL (angle brackets allowed), an optional quoted title (shown on
/// hover), and how many bytes up to and including the closing `)`.
fn masked_target(after: &str) -> Option<(&str, Option<&str>, usize)> {
    let lead = after.len() - after.trim_start().len();
    let body = &after[lead..];
    let url_len = body.find(|c: char| c.is_whitespace() || c == ')')?;
    let url = body[..url_len].trim_start_matches('<').trim_end_matches('>');
    let mut pos = lead + url_len;
    pos += after[pos..].len() - after[pos..].trim_start().len();
    let mut title = None;
    if let Some(q) = after[pos..].chars().next().filter(|&q| q == '"' || q == '\'') {
        let end = after[pos + 1..].find(q)?;
        title = Some(&after[pos + 1..pos + 1 + end]);
        pos += end + 2;
        pos += after[pos..].len() - after[pos..].trim_start().len();
    }
    after[pos..].starts_with(')').then_some((url, title, pos + 1))
}

/// A masked link's text, without the code ticks or bold stars around it (Discord shows those as formatting).
fn link_label(mut label: &str, mut style: Style) -> (&str, Style) {
    loop {
        if label.len() > 4 && label.starts_with("**") && label.ends_with("**") {
            style.bold = true;
            label = &label[2..label.len() - 2];
        } else if label.len() > 2 && (label.starts_with('`') && label.ends_with('`') || label.starts_with('*') && label.ends_with('*')) {
            if label.starts_with('*') {
                style.italic = true;
            }
            label = &label[1..label.len() - 1];
        } else {
            return (label, style);
        }
    }
}

fn is_url(s: &str) -> bool {
    (s.starts_with("https://") || s.starts_with("http://")) && !s.contains(char::is_whitespace)
}

fn angle_span(inner: &str) -> Option<Span> {
    let id = |s: &str| s.parse::<u64>().ok().map(Snowflake);
    if let Some(r) = inner.strip_prefix("@&") {
        return id(r).map(Span::Role);
    }
    if let Some(r) = inner.strip_prefix("@!").or_else(|| inner.strip_prefix('@')) {
        return id(r).map(Span::User);
    }
    if let Some(r) = inner.strip_prefix('#') {
        return id(r).map(Span::Channel);
    }
    if let Some(r) = inner.strip_prefix("t:") {
        let (ts, style) = r.split_once(':').map_or((r, 'f'), |(t, s)| (t, s.chars().next().unwrap_or('f')));
        return ts.parse().ok().map(|t| Span::Timestamp(t, style));
    }
    let (animated, r) = match inner.strip_prefix("a:") {
        Some(r) => (true, r),
        None => (false, inner.strip_prefix(':')?),
    };
    let (name, eid) = r.split_once(':')?;
    if name.is_empty() {
        return None;
    }
    Some(Span::Emoji { id: id(eid)?, name: name.to_owned(), animated })
}

/// Unicode emoji, approximately: pictographic code points, flags and keycaps.
pub fn is_emoji_grapheme(g: &str) -> bool {
    let mut chars = g.chars();
    let Some(first) = chars.next() else { return false };
    let cp = first as u32;
    // Keycaps: digit/#/* + FE0F + 20E3.
    if g.contains('\u{20E3}') {
        return true;
    }
    // Regional indicators: pairs are flags, and Discord shows a lone one as a letter tile.
    if (0x1F1E6..=0x1F1FF).contains(&cp) {
        return chars.next().is_none_or(|c| (0x1F1E6..=0x1F1FF).contains(&(c as u32)));
    }
    // The supplementary planes' emoji blocks (pictographs, emoticons, transport, geometric shapes like 🟢,
    // supplemental symbols…), minus skin-tone modifiers on their own.
    let pictographic = matches!(cp, 0x1F000..=0x1FAFF) && !(0x1F3FB..=0x1F3FF).contains(&cp);
    // Older symbols are only emoji when they're followed by the emoji presentation selector.
    let symbol = matches!(cp, 0x2600..=0x27BF | 0x2300..=0x23FF | 0x2B00..=0x2BFF | 0x00A9 | 0x00AE | 0x203C | 0x2049
        | 0x2122 | 0x2139 | 0x2194..=0x21AA | 0x24C2 | 0x25AA..=0x25FE | 0x2934 | 0x2935 | 0x3030 | 0x303D | 0x3297 | 0x3299);
    // A skin tone also makes one an emoji (✌🏽). Discord writes ⚧ without the selector, and shows © ® ™ as
    // emoji too.
    let toned = g.chars().any(|c| (0x1F3FB..=0x1F3FF).contains(&(c as u32)));
    let discord = matches!(cp, 0x26A7 | 0xA9 | 0xAE | 0x2122);
    pictographic || (symbol && (g.contains('\u{FE0F}') || toned || discord || is_default_emoji_symbol(cp)))
}

/// Symbols in the BMP that render as emoji by default (Emoji_Presentation=Yes).
fn is_default_emoji_symbol(cp: u32) -> bool {
    matches!(cp,
        0x231A | 0x231B | 0x23E9..=0x23EC | 0x23F0 | 0x23F3 | 0x25FD | 0x25FE | 0x2614 | 0x2615 | 0x2648..=0x2653
        | 0x267F | 0x2693 | 0x26A1 | 0x26AA | 0x26AB | 0x26BD | 0x26BE | 0x26C4 | 0x26C5 | 0x26CE | 0x26D4 | 0x26EA
        | 0x26F2 | 0x26F3 | 0x26F5 | 0x26FA | 0x26FD | 0x2705 | 0x270A | 0x270B | 0x2728 | 0x274C | 0x274E
        | 0x2753..=0x2755 | 0x2757 | 0x2795..=0x2797 | 0x27B0 | 0x27BF | 0x2B1B | 0x2B1C | 0x2B50 | 0x2B55)
}

/// The Twemoji asset name for an emoji: code points in hex joined by '-', without FE0F unless it's a ZWJ
/// sequence.
pub fn twemoji_name(emoji: &str) -> String {
    // The eye in a speech bubble is the exception: Twemoji names it without its selectors.
    if emoji.replace('\u{FE0F}', "") == "\u{1F441}\u{200D}\u{1F5E8}" {
        return "1f441-200d-1f5e8".into();
    }
    let keep_fe0f = emoji.contains('\u{200D}');
    emoji
        .chars()
        .filter(|&c| keep_fe0f || c != '\u{FE0F}')
        .map(|c| format!("{:x}", c as u32))
        .collect::<Vec<_>>()
        .join("-")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spans(s: &str) -> Vec<Span> {
        let [Block::Paragraph(p)] = &parse(s)[..] else { panic!("{:?}", parse(s)) };
        p.clone()
    }

    fn t(s: &str, f: impl FnOnce(&mut Style)) -> Span {
        let mut st = Style::default();
        f(&mut st);
        Span::Text(s.into(), st)
    }

    #[test]
    fn styles_nest() {
        assert_eq!(
            spans("a **b *c*** d"),
            vec![t("a ", |_| {}), t("b ", |s| s.bold = true), t("c", |s| { s.bold = true; s.italic = true }), t(" d", |_| {})]
        );
        assert_eq!(spans("||x|| ||y||")[2], t("y", |s| s.spoiler = Some(1)));
    }

    #[test]
    fn literal_stars_and_snake_case_survive() {
        assert_eq!(spans("2 * 3 * 4"), vec![t("2 * 3 * 4", |_| {})]);
        assert_eq!(spans("my_var_name"), vec![t("my_var_name", |_| {})]);
        assert_eq!(spans(r"\*not italic\*"), vec![t("*not italic*", |_| {})]);
    }

    #[test]
    fn mentions_emoji_and_links() {
        let s = spans("hi <@123> <#5> <:blob:77> see https://x.com/a). [site](https://y.org)");
        assert!(s.contains(&Span::User(Snowflake(123))));
        assert!(s.contains(&Span::Channel(Snowflake(5))));
        assert!(s.contains(&Span::Emoji { id: Snowflake(77), name: "blob".into(), animated: false }));
        assert!(s.contains(&Span::Link { text: "https://x.com/a".into(), url: "https://x.com/a".into(), style: Style::default(), title: None }));
        assert!(s.contains(&Span::Link { text: "site".into(), url: "https://y.org".into(), style: Style::default(), title: None }));
    }

    #[test]
    fn masked_links_with_titles_and_formatting() {
        // As a bot (owo's osu! profile embed) writes them.
        let s = spans("Accuracy: [`99.37%`](https://discord.com/channels/1/2/# \"99.3695\") • [**Link to skin**](https://drive.google.com/x 'drive')");
        let link = |text: &str, url: &str, title: &str, bold: bool| Span::Link {
            text: text.into(),
            url: url.into(),
            style: Style { bold, ..Default::default() },
            title: Some(title.into()),
        };
        assert!(s.contains(&link("99.37%", "https://discord.com/channels/1/2/#", "99.3695", false)), "{s:?}");
        assert!(s.contains(&link("Link to skin", "https://drive.google.com/x", "drive", true)), "{s:?}");
        // A title with a bracket in it, and angle brackets round the URL.
        let s = spans("[a](<https://z.org> \"x (y)\") after");
        assert_eq!(s[0], link("a", "https://z.org", "x (y)", false));
        assert_eq!(s[1], Span::Text(" after".into(), Style::default()));
        // Not a masked link with an unclosed title (the bare URL in it is still linked, as in Discord).
        assert!(!spans("[a](https://z.org \"open").iter().any(|s| matches!(s, Span::Link { text, .. } if text == "a")));
    }

    #[test]
    fn blocks() {
        let b = parse("# Title\n> quoted\n> more\n- item\n```rs\nlet x = 1;\n```\nafter");
        assert!(matches!(&b[0], Block::Heading(1, _)));
        assert!(matches!(&b[1], Block::Quote(q) if q.len() == 1));
        assert!(matches!(&b[2], Block::ListItem { depth: 0, .. }));
        assert_eq!(b[3], Block::Code { lang: Some("rs".into()), code: "let x = 1;".into() });
        assert!(matches!(&b[4], Block::Paragraph(_)));
    }

    #[test]
    fn emoji_detection() {
        assert!(is_emoji_only(&parse("😀 👍🏽")));
        assert!(!is_emoji_only(&parse("😀 hi")));
        assert!(is_emoji_only(&parse("🇯🇵")));
        // As in Discord, © is an emoji; arrows without the selector are text.
        assert!(is_emoji_only(&parse("©")));
        assert!(!is_emoji_only(&parse("↔")));
        assert!(is_emoji_only(&parse("🟢 🟡")));
        assert!(is_emoji_only(&parse("▶️ ◼️ ⤴️ Ⓜ️ 🇦")));
        assert_eq!(twemoji_name("👁️‍🗨️"), "1f441-200d-1f5e8");
        assert_eq!(twemoji_name("👍🏽"), "1f44d-1f3fd");
        assert_eq!(twemoji_name("❤️"), "2764");
        assert_eq!(twemoji_name("👨‍👩‍👧"), "1f468-200d-1f469-200d-1f467");
    }

    #[test]
    fn timestamps() {
        assert_eq!(spans("<t:1700000000:R>"), vec![Span::Timestamp(1700000000, 'R')]);
    }
}
