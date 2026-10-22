using Swiftcord.Core.Markdown;

namespace Swiftcord.Core.Tests.Markdown;

public sealed class MarkdownParserTests
{
    /// <summary>Renders the AST compactly, e.g. (b "bold") "text".</summary>
    private static string Format(IReadOnlyList<MarkdownNode> nodes) => string.Join(" ", nodes.Select(Format));

    private static string Format(MarkdownNode node) => node switch
    {
        TextNode t => $"\"{t.Text.Replace("\n", "\\n")}\"",
        BoldNode n => $"(b {Format(n.Children)})",
        ItalicNode n => $"(i {Format(n.Children)})",
        UnderlineNode n => $"(u {Format(n.Children)})",
        StrikethroughNode n => $"(s {Format(n.Children)})",
        SpoilerNode n => $"(spoiler {Format(n.Children)})",
        BlockQuoteNode n => $"(quote {Format(n.Children)})",
        HeadingNode n => $"(h{n.Level} {Format(n.Children)})",
        SubtextNode n => $"(sub {Format(n.Children)})",
        ListNode n => $"({(n.Ordered ? $"ol{n.Start}" : "ul")} {string.Join(" ", n.Items.Select(i => $"[{Format(i)}]"))})",
        InlineCodeNode n => $"(code \"{n.Code}\")",
        CodeBlockNode n => $"(codeblock {n.Language ?? "-"} \"{n.Code.Replace("\n", "\\n")}\")",
        LinkNode n => $"(link{(n.SuppressEmbed ? "!" : "")} {n.Url} {Format(n.Children)})",
        UserMentionNode n => $"(@{n.UserId})",
        RoleMentionNode n => $"(@&{n.RoleId})",
        ChannelMentionNode n => $"(#{n.ChannelId})",
        EveryoneMentionNode n => n.Here ? "(@here)" : "(@everyone)",
        CustomEmojiNode n => $"(emoji{(n.Animated ? "-a" : "")} {n.Name} {n.Id})",
        TimestampNode n => $"(t {n.Time.ToUnixTimeSeconds()} {n.Style})",
        _ => throw new NotSupportedException(node.GetType().Name),
    };

    private static string Parse(string input) => Format(MarkdownParser.Parse(input));

    [Theory]
    [InlineData("plain text", "\"plain text\"")]
    [InlineData("**bold** and *it* and _it2_", "(b \"bold\") \" and \" (i \"it\") \" and \" (i \"it2\")")]
    [InlineData("__under__ ~~gone~~ ||secret||", "(u \"under\") \" \" (s \"gone\") \" \" (spoiler \"secret\")")]
    [InlineData("***both***", "(b (i \"both\"))")]
    [InlineData("**bold *nested* bold**", "(b \"bold \" (i \"nested\") \" bold\")")]
    [InlineData("snake_case_name", "\"snake_case_name\"")]
    [InlineData("2 * 3 * 4", "\"2 * 3 * 4\"")]
    [InlineData(@"\*not italic\*", "\"*not italic*\"")]
    public void Inline_formatting(string input, string expected) => Assert.Equal(expected, Parse(input));

    [Theory]
    [InlineData("use `**raw**` here", "\"use \" (code \"**raw**\") \" here\"")]
    [InlineData("``code with ` tick``", "(code \"code with ` tick\")")]
    [InlineData("```js\nconsole.log(1)\n```", "(codeblock js \"console.log(1)\")")]
    [InlineData("```no language```", "(codeblock - \"no language\")")]
    [InlineData("```\n**not bold**\n```", "(codeblock - \"**not bold**\")")]
    public void Code_is_not_formatted(string input, string expected) => Assert.Equal(expected, Parse(input));

    [Theory]
    [InlineData("see https://swiftcord.app.", "\"see \" (link https://swiftcord.app \"https://swiftcord.app\") \".\"")]
    [InlineData("<https://example.com/a>", "(link! https://example.com/a \"https://example.com/a\")")]
    [InlineData("[the **docs**](https://example.com)", "(link https://example.com \"the \" (b \"docs\"))")]
    [InlineData("[bad](javascript:alert(1))", "\"[bad](javascript:alert(1))\"")]
    public void Links(string input, string expected) => Assert.Equal(expected, Parse(input));

    [Theory]
    [InlineData("hi <@123> <@!456>", "\"hi \" (@123) \" \" (@456)")]
    [InlineData("<@&789> in <#1011>", "(@&789) \" in \" (#1011)")]
    [InlineData("@everyone @here", "(@everyone) \" \" (@here)")]
    [InlineData("me@everyone.com", "\"me\" (@everyone) \".com\"")]
    [InlineData("<:blob:111> <a:dance:222>", "(emoji blob 111) \" \" (emoji-a dance 222)")]
    [InlineData("<t:1700000000:R> <t:1700000000>", "(t 1700000000 R) \" \" (t 1700000000 f)")]
    public void Mentions_emoji_and_timestamps(string input, string expected) => Assert.Equal(expected, Parse(input));

    [Theory]
    [InlineData("> quoted\n> more\nafter", "(quote \"quoted\\nmore\") \"after\"")]
    [InlineData(">>> everything\nelse **too**", "(quote \"everything\\nelse \" (b \"too\"))")]
    [InlineData("not > a quote", "\"not > a quote\"")]
    [InlineData("# Big\n## Medium\ntext", "(h1 \"Big\") (h2 \"Medium\") \"text\"")]
    [InlineData("#nospace", "\"#nospace\"")]
    [InlineData("-# small print", "(sub \"small print\")")]
    [InlineData("- one\n- **two**\nend", "(ul [\"one\"] [(b \"two\")]) \"end\"")]
    [InlineData("3. three\n4. four", "(ol3 [\"three\"] [\"four\"])")]
    [InlineData("> # heading in quote", "(quote (h1 \"heading in quote\"))")]
    public void Block_elements(string input, string expected) => Assert.Equal(expected, Parse(input));

    [Fact]
    public void Block_rules_only_apply_at_line_start()
    {
        Assert.Equal("\"text # not heading\"", Parse("text # not heading"));
        Assert.Equal("\"line\\n\" (h3 \"head\")", Parse("line\n### head"));
    }

    [Fact]
    public void Deeply_nested_input_does_not_overflow()
    {
        var input = string.Concat(Enumerable.Repeat("||", 200)) + "x" + string.Concat(Enumerable.Repeat("||", 200));
        Assert.NotEmpty(MarkdownParser.Parse(input));
    }

    [Theory]
    [InlineData("😀", true)]
    [InlineData("👍🏽 ❤️ 👨‍👩‍👧", true)]
    [InlineData("<:blob:111><a:dance:222>", true)]
    [InlineData("hi 😀", false)]
    [InlineData("**😀**", false)]
    [InlineData("", false)]
    public void Detects_emoji_only_messages(string input, bool expected) =>
        Assert.Equal(expected, MarkdownParser.IsEmojiOnly(MarkdownParser.Parse(input)));
}
