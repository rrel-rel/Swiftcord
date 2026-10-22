namespace Swiftcord.Core.Markdown;

public abstract record MarkdownNode;

/// <summary>A node whose content is itself parsed markdown.</summary>
public abstract record ContainerNode(IReadOnlyList<MarkdownNode> Children) : MarkdownNode;

public sealed record TextNode(string Text) : MarkdownNode;

public sealed record BoldNode(IReadOnlyList<MarkdownNode> Children) : ContainerNode(Children);

public sealed record ItalicNode(IReadOnlyList<MarkdownNode> Children) : ContainerNode(Children);

public sealed record UnderlineNode(IReadOnlyList<MarkdownNode> Children) : ContainerNode(Children);

public sealed record StrikethroughNode(IReadOnlyList<MarkdownNode> Children) : ContainerNode(Children);

public sealed record SpoilerNode(IReadOnlyList<MarkdownNode> Children) : ContainerNode(Children);

public sealed record BlockQuoteNode(IReadOnlyList<MarkdownNode> Children) : ContainerNode(Children);

/// <param name="Level">1–3.</param>
public sealed record HeadingNode(int Level, IReadOnlyList<MarkdownNode> Children) : ContainerNode(Children);

/// <summary>Small, dimmed text written with a "-# " prefix.</summary>
public sealed record SubtextNode(IReadOnlyList<MarkdownNode> Children) : ContainerNode(Children);

public sealed record ListNode(bool Ordered, int Start, IReadOnlyList<IReadOnlyList<MarkdownNode>> Items) : MarkdownNode;

public sealed record InlineCodeNode(string Code) : MarkdownNode;

public sealed record CodeBlockNode(string? Language, string Code) : MarkdownNode;

/// <param name="SuppressEmbed">True for &lt;url&gt; links, which Discord doesn't unfurl.</param>
public sealed record LinkNode(string Url, IReadOnlyList<MarkdownNode> Children, bool SuppressEmbed = false) : ContainerNode(Children);

public sealed record UserMentionNode(Snowflake UserId) : MarkdownNode;

public sealed record RoleMentionNode(Snowflake RoleId) : MarkdownNode;

public sealed record ChannelMentionNode(Snowflake ChannelId) : MarkdownNode;

/// <summary>@everyone, or @here when <paramref name="Here"/> is true.</summary>
public sealed record EveryoneMentionNode(bool Here) : MarkdownNode;

public sealed record CustomEmojiNode(string Name, Snowflake Id, bool Animated) : MarkdownNode;

/// <param name="Style">One of t, T, d, D, f, F, s, S, R (relative). Defaults to f.</param>
public sealed record TimestampNode(DateTimeOffset Time, char Style) : MarkdownNode;
