using System.Globalization;
using System.Text;
using System.Text.RegularExpressions;

namespace Swiftcord.Core.Markdown;

/// <summary>
/// Parses Discord-flavoured markdown into an AST. Modelled on the rule order of simple-markdown,
/// which Discord's own client uses: at each position, the first matching rule wins and anything
/// else is plain text.
/// </summary>
public static partial class MarkdownParser
{
    private const int MaxDepth = 24;

    public static IReadOnlyList<MarkdownNode> Parse(string text) =>
        ParseCore(text.Replace("\r\n", "\n"), allowBlocks: true, allowQuote: true, depth: 0);

    /// <summary>
    /// Whether a message consists only of emoji (at most 30), which Discord renders large.
    /// </summary>
    public static bool IsEmojiOnly(IReadOnlyList<MarkdownNode> nodes)
    {
        var count = 0;
        foreach (var node in nodes)
        {
            switch (node)
            {
                case CustomEmojiNode:
                    count++;
                    break;
                case TextNode { Text: var text }:
                    foreach (var element in EnumerateTextElements(text))
                    {
                        if (string.IsNullOrWhiteSpace(element)) continue;
                        if (!IsEmojiElement(element)) return false;
                        count++;
                    }
                    break;
                default:
                    return false;
            }
        }
        return count is > 0 and <= 30;
    }

    private static IReadOnlyList<MarkdownNode> ParseCore(string s, bool allowBlocks, bool allowQuote, int depth)
    {
        var nodes = new List<MarkdownNode>();
        if (depth > MaxDepth)
        {
            nodes.Add(new TextNode(s));
            return nodes;
        }

        var text = new StringBuilder();
        void Flush()
        {
            if (text.Length == 0) return;
            nodes.Add(new TextNode(text.ToString()));
            text.Clear();
        }
        void Emit(MarkdownNode node)
        {
            Flush();
            nodes.Add(node);
        }
        IReadOnlyList<MarkdownNode> Inner(string content, bool blocks = false, bool quote = false) =>
            ParseCore(content, blocks, quote, depth + 1);

        var pos = 0;
        while (pos < s.Length)
        {
            Match m;
            var atLineStart = pos == 0 || s[pos - 1] == '\n';

            if (allowBlocks && atLineStart)
            {
                if (allowQuote && (m = BlockQuoteRestRegex().Match(s, pos)).Success)
                {
                    Emit(new BlockQuoteNode(Inner(m.Groups[1].Value, blocks: true)));
                    pos += m.Length;
                    continue;
                }
                if (allowQuote && (m = BlockQuoteLinesRegex().Match(s, pos)).Success)
                {
                    var content = string.Join('\n', m.Value.TrimEnd('\n').Split('\n').Select(line => line[2..]));
                    Emit(new BlockQuoteNode(Inner(content, blocks: true)));
                    pos += m.Length;
                    continue;
                }
                if ((m = HeadingRegex().Match(s, pos)).Success)
                {
                    Emit(new HeadingNode(m.Groups[1].Length, Inner(m.Groups[2].Value.Trim())));
                    pos += m.Length;
                    continue;
                }
                if ((m = SubtextRegex().Match(s, pos)).Success)
                {
                    Emit(new SubtextNode(Inner(m.Groups[1].Value.Trim())));
                    pos += m.Length;
                    continue;
                }
                if ((m = ListRegex().Match(s, pos)).Success)
                {
                    Emit(ParseList(m.Value, depth));
                    pos += m.Length;
                    continue;
                }
            }

            if ((m = CodeBlockRegex().Match(s, pos)).Success)
            {
                Emit(new CodeBlockNode(m.Groups[1].Success ? m.Groups[1].Value : null, m.Groups[2].Value));
                pos += m.Length;
            }
            else if ((m = InlineCodeRegex().Match(s, pos)).Success)
            {
                Emit(new InlineCodeNode(m.Groups[2].Value));
                pos += m.Length;
            }
            else if ((m = EscapeRegex().Match(s, pos)).Success)
            {
                text.Append(m.Groups[1].Value);
                pos += m.Length;
            }
            else if ((m = SpoilerRegex().Match(s, pos)).Success)
            {
                Emit(new SpoilerNode(Inner(m.Groups[1].Value)));
                pos += m.Length;
            }
            else if ((m = BoldRegex().Match(s, pos)).Success)
            {
                Emit(new BoldNode(Inner(m.Groups[1].Value)));
                pos += m.Length;
            }
            else if ((m = UnderlineRegex().Match(s, pos)).Success)
            {
                Emit(new UnderlineNode(Inner(m.Groups[1].Value)));
                pos += m.Length;
            }
            else if ((m = ItalicRegex().Match(s, pos)).Success)
            {
                var content = m.Groups[1].Success ? m.Groups[1].Value : m.Groups[2].Value;
                Emit(new ItalicNode(Inner(content)));
                pos += m.Length;
            }
            else if ((m = StrikethroughRegex().Match(s, pos)).Success)
            {
                Emit(new StrikethroughNode(Inner(m.Groups[1].Value)));
                pos += m.Length;
            }
            else if ((m = MaskedLinkRegex().Match(s, pos)).Success)
            {
                Emit(new LinkNode(m.Groups[2].Value, Inner(m.Groups[1].Value)));
                pos += m.Length;
            }
            else if ((m = AngleLinkRegex().Match(s, pos)).Success)
            {
                Emit(new LinkNode(m.Groups[1].Value, [new TextNode(m.Groups[1].Value)], SuppressEmbed: true));
                pos += m.Length;
            }
            else if ((m = UrlRegex().Match(s, pos)).Success)
            {
                Emit(new LinkNode(m.Value, [new TextNode(m.Value)]));
                pos += m.Length;
            }
            else if ((m = UserMentionRegex().Match(s, pos)).Success)
            {
                Emit(new UserMentionNode(Snowflake.Parse(m.Groups[1].Value)));
                pos += m.Length;
            }
            else if ((m = RoleMentionRegex().Match(s, pos)).Success)
            {
                Emit(new RoleMentionNode(Snowflake.Parse(m.Groups[1].Value)));
                pos += m.Length;
            }
            else if ((m = ChannelMentionRegex().Match(s, pos)).Success)
            {
                Emit(new ChannelMentionNode(Snowflake.Parse(m.Groups[1].Value)));
                pos += m.Length;
            }
            else if ((m = CustomEmojiRegex().Match(s, pos)).Success)
            {
                Emit(new CustomEmojiNode(m.Groups[2].Value, Snowflake.Parse(m.Groups[3].Value), m.Groups[1].Length > 0));
                pos += m.Length;
            }
            else if ((m = TimestampRegex().Match(s, pos)).Success
                && long.TryParse(m.Groups[1].Value, NumberStyles.AllowLeadingSign, CultureInfo.InvariantCulture, out var seconds)
                && seconds is >= -62135596800 and <= 253402300799)
            {
                var style = m.Groups[2].Success ? m.Groups[2].Value[0] : 'f';
                Emit(new TimestampNode(DateTimeOffset.FromUnixTimeSeconds(seconds), style));
                pos += m.Length;
            }
            else if ((m = EveryoneRegex().Match(s, pos)).Success)
            {
                Emit(new EveryoneMentionNode(m.Groups[1].Value == "here"));
                pos += m.Length;
            }
            else if ((m = TextRegex().Match(s, pos)).Success && m.Length > 0)
            {
                text.Append(m.Value);
                pos += m.Length;
            }
            else
            {
                text.Append(s[pos]);
                pos++;
            }
        }

        Flush();
        return nodes;
    }

    private static ListNode ParseList(string block, int depth)
    {
        var items = new List<IReadOnlyList<MarkdownNode>>();
        bool? ordered = null;
        var start = 1;
        foreach (var line in block.TrimEnd('\n').Split('\n'))
        {
            var m = ListItemRegex().Match(line);
            var marker = m.Groups[1].Value;
            if (ordered is null)
            {
                ordered = char.IsAsciiDigit(marker[0]);
                if (ordered.Value)
                    start = int.Parse(marker.TrimEnd('.'), CultureInfo.InvariantCulture);
            }
            items.Add(ParseCore(m.Groups[2].Value, allowBlocks: false, allowQuote: false, depth + 1));
        }
        return new ListNode(ordered ?? false, start, items);
    }

    private static IEnumerable<string> EnumerateTextElements(string text)
    {
        var e = StringInfo.GetTextElementEnumerator(text);
        while (e.MoveNext())
            yield return (string)e.Current;
    }

    private static bool IsEmojiElement(string element)
    {
        var rune = Rune.GetRuneAt(element, 0);
        var v = rune.Value;
        // Pictographic planes, dingbats, misc symbols, regional indicators, and keycaps (e.g. 1️⃣).
        return v is >= 0x1F000 and <= 0x1FAFF
            or >= 0x2600 and <= 0x27BF
            or >= 0x2300 and <= 0x23FF
            or >= 0x2B00 and <= 0x2BFF
            or 0x00A9 or 0x00AE or 0x203C or 0x2049 or 0x2122 or 0x2139 or 0x3030 or 0x303D or 0x3297 or 0x3299
            || (element.Length > 1 && element.Contains('⃣'));
    }

    // ── Block rules (only at the start of a line) ──────────────────────────────

    [GeneratedRegex(@"\G>>> ?([\s\S]*)")]
    private static partial Regex BlockQuoteRestRegex();

    [GeneratedRegex(@"\G(?:> [^\n]*(?:\n|$))+")]
    private static partial Regex BlockQuoteLinesRegex();

    [GeneratedRegex(@"\G(#{1,3}) +([^\n]+)(?:\n|$)")]
    private static partial Regex HeadingRegex();

    [GeneratedRegex(@"\G-# +([^\n]+)(?:\n|$)")]
    private static partial Regex SubtextRegex();

    [GeneratedRegex(@"\G(?: *(?:[-*]|\d{1,9}\.) +[^\n]+(?:\n|$))+")]
    private static partial Regex ListRegex();

    [GeneratedRegex(@"^ *([-*]|\d{1,9}\.) +(.*)$")]
    private static partial Regex ListItemRegex();

    // ── Inline rules ───────────────────────────────────────────────────────────

    [GeneratedRegex(@"\G```(?:([A-Za-z0-9_+\-.#]+?)\n)?\n*([^\n][\s\S]*?)\n*```")]
    private static partial Regex CodeBlockRegex();

    [GeneratedRegex(@"\G(`+)([\s\S]*?[^`])\1(?!`)")]
    private static partial Regex InlineCodeRegex();

    [GeneratedRegex(@"\G\\([^0-9A-Za-z\s])")]
    private static partial Regex EscapeRegex();

    [GeneratedRegex(@"\G\|\|([\s\S]+?)\|\|")]
    private static partial Regex SpoilerRegex();

    [GeneratedRegex(@"\G\*\*([\s\S]+?)\*\*(?!\*)")]
    private static partial Regex BoldRegex();

    [GeneratedRegex(@"\G__([\s\S]+?)__(?!_)")]
    private static partial Regex UnderlineRegex();

    // _underscore_ italics need word boundaries (so snake_case_names stay plain); *star* italics
    // must not start or end with whitespace.
    [GeneratedRegex(@"\G(?:\b_((?:__|\\[\s\S]|[^\\_])+?)_\b|\*(?=\S)((?:\*\*|\\[\s\S]|\s+(?:\\[\s\S]|[^\s*\\]|\*\*)|[^\s*\\])+?)\*(?!\*))")]
    private static partial Regex ItalicRegex();

    [GeneratedRegex(@"\G~~([\s\S]+?)~~")]
    private static partial Regex StrikethroughRegex();

    [GeneratedRegex(@"\G\[((?:\[[^\]]*\]|[^\[\]])*)\]\(\s*<?(https?://[^\s>)]+)>?\s*\)")]
    private static partial Regex MaskedLinkRegex();

    [GeneratedRegex(@"\G<(https?://[^\s>]+)>")]
    private static partial Regex AngleLinkRegex();

    [GeneratedRegex(@"\Ghttps?://[^\s<]+[^<.,:;""')\]\s]")]
    private static partial Regex UrlRegex();

    [GeneratedRegex(@"\G<@!?(\d{1,20})>")]
    private static partial Regex UserMentionRegex();

    [GeneratedRegex(@"\G<@&(\d{1,20})>")]
    private static partial Regex RoleMentionRegex();

    [GeneratedRegex(@"\G<#(\d{1,20})>")]
    private static partial Regex ChannelMentionRegex();

    [GeneratedRegex(@"\G<(a?):(\w{2,32}):(\d{1,20})>")]
    private static partial Regex CustomEmojiRegex();

    [GeneratedRegex(@"\G<t:(-?\d{1,17})(?::([tTdDfFsSR]))?>")]
    private static partial Regex TimestampRegex();

    [GeneratedRegex(@"\G@(everyone|here)\b")]
    private static partial Regex EveryoneRegex();

    // Consume plain text up to the next character that could start another rule.
    [GeneratedRegex(@"\G[\s\S]+?(?=[^0-9A-Za-z\sÀ-￿]|\n|\w+:\S|$)")]
    private static partial Regex TextRegex();
}
