//! Mentions while composing: finding the `@`/`#`/`:` token being typed, and turning the readable `@name`
//! text in the composer into Discord's `<@id>` syntax when sending.

/// What's being typed at the caret.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    /// Byte offset of the trigger character.
    pub start: usize,
    pub trigger: char,
    /// The text after the trigger, up to the caret.
    pub query: String,
}

/// The mention/channel/emoji token ending at `caret` (a byte offset), if any. The trigger must start the text
/// or follow whitespace, and nothing between it and the caret may be whitespace. Emoji need two characters
/// first, so "a:b" or a lone colon don't pop up suggestions.
pub fn token_at(text: &str, caret: usize) -> Option<Token> {
    let before = text.get(..caret)?;
    let start = before.rfind(|c: char| c.is_whitespace()).map_or(0, |i| i + before[i..].chars().next().map_or(1, char::len_utf8));
    let word = &before[start..];
    let trigger = word.chars().next()?;
    if !matches!(trigger, '@' | '#' | ':') {
        return None;
    }
    let query = &word[trigger.len_utf8()..];
    if trigger == ':' && (query.chars().count() < 2 || query.contains(':')) {
        return None;
    }
    if query.contains(['@', '#']) {
        return None;
    }
    Some(Token { start, trigger, query: query.to_owned() })
}

/// Rewrites a message for sending: each readable mention inserted by picking a suggestion (`display` →
/// `raw`, e.g. "@alice" → "<@123>") is replaced where it stands as a whole word, then any remaining `@word` is
/// offered to `resolve` (for names typed out in full).
pub fn finalize(text: &str, picked: &[(String, String)], resolve: impl Fn(&str) -> Option<String>) -> String {
    let mut picked: Vec<&(String, String)> = picked.iter().collect();
    // Longest first, so "@al" can't clobber part of "@alice".
    picked.sort_by_key(|(d, _)| std::cmp::Reverse(d.len()));
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    let at_word_start = |s: &str, i: usize| i == 0 || s[..i].ends_with(char::is_whitespace);
    let ends_word = |s: &str, j: usize| s[j..].chars().next().is_none_or(|c| c.is_whitespace() || c.is_ascii_punctuation() && c != '_' && c != '.');
    'outer: while i < text.len() {
        if at_word_start(text, i) {
            for (display, raw) in &picked {
                if text[i..].starts_with(display.as_str()) && ends_word(text, i + display.len()) {
                    out.push_str(raw);
                    i += display.len();
                    continue 'outer;
                }
            }
            if text[i..].starts_with('@') {
                let end = text[i + 1..].find(|c: char| c.is_whitespace() || (c.is_ascii_punctuation() && c != '_' && c != '.')).map_or(text.len(), |e| i + 1 + e);
                let name = &text[i + 1..end];
                if !name.is_empty() && name != "everyone" && name != "here" {
                    if let Some(raw) = resolve(name) {
                        out.push_str(&raw);
                        i = end;
                        continue;
                    }
                }
            }
        }
        let c = text[i..].chars().next().unwrap();
        out.push(c);
        i += c.len_utf8();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tok(t: char, start: usize, q: &str) -> Option<Token> {
        Some(Token { start, trigger: t, query: q.into() })
    }

    #[test]
    fn finds_tokens_at_the_caret() {
        assert_eq!(token_at("hi @al", 6), tok('@', 3, "al"));
        assert_eq!(token_at("@", 1), tok('@', 0, ""));
        assert_eq!(token_at("see #gen", 8), tok('#', 4, "gen"));
        assert_eq!(token_at("nice :th", 8), tok(':', 5, "th"));
        assert_eq!(token_at("nice :t", 7), None, "emoji need two characters");
        assert_eq!(token_at("mail a@b", 8), None, "not at a word start");
        assert_eq!(token_at("@al done", 8), None, "caret past the token");
        assert_eq!(token_at("é @ü", "é @ü".len()), tok('@', "é ".len(), "ü"));
    }

    #[test]
    fn finalizes_picked_and_typed_mentions() {
        let picked = vec![("@al".to_owned(), "<@1>".to_owned()), ("@alice".to_owned(), "<@2>".to_owned()), ("#general".to_owned(), "<#9>".to_owned())];
        let resolve = |n: &str| (n == "bob").then(|| "<@3>".to_owned());
        assert_eq!(finalize("@alice and @al, see #general", &picked, resolve), "<@2> and <@1>, see <#9>");
        assert_eq!(finalize("hey @bob! @carol", &picked, resolve), "hey <@3>! @carol");
        assert_eq!(finalize("email me@bob.com @everyone", &picked, resolve), "email me@bob.com @everyone");
    }
}
