using System.Text;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Documents;
using Microsoft.UI.Xaml.Media;
using Microsoft.UI.Xaml.Media.Imaging;
using Swiftcord.App.Helpers;
using Swiftcord.Core.Markdown;
using Swiftcord.ViewModels.Messages;
using Swiftcord.ViewModels.Navigation;

namespace Swiftcord.App.Controls;

/// <summary>
/// Renders a Discord markdown AST. Inline formatting goes into RichTextBlock paragraphs;
/// code blocks, quotes, headings and lists become their own elements in a vertical stack.
/// </summary>
public sealed partial class MarkdownTextBlock : ContentControl
{
    public static readonly DependencyProperty NodesProperty = DependencyProperty.Register(
        nameof(Nodes), typeof(object), typeof(MarkdownTextBlock), new PropertyMetadata(null, OnChanged));

    public static readonly DependencyProperty MessageContextProperty = DependencyProperty.Register(
        nameof(MessageContext), typeof(object), typeof(MarkdownTextBlock), new PropertyMetadata(null, OnChanged));

    public static readonly DependencyProperty IsEmojiOnlyProperty = DependencyProperty.Register(
        nameof(IsEmojiOnly), typeof(bool), typeof(MarkdownTextBlock), new PropertyMetadata(false, OnChanged));

    public static readonly DependencyProperty IsEditedProperty = DependencyProperty.Register(
        nameof(IsEdited), typeof(bool), typeof(MarkdownTextBlock), new PropertyMetadata(false, OnChanged));

    public MarkdownTextBlock()
    {
        IsTabStop = false;
        HorizontalContentAlignment = HorizontalAlignment.Stretch;
    }

    /// <summary>The parsed message (an IReadOnlyList&lt;MarkdownNode&gt;). Set it after the other properties.</summary>
    public object? Nodes
    {
        get => GetValue(NodesProperty);
        set => SetValue(NodesProperty, value);
    }

    /// <summary>An <see cref="IMessageContext"/> for resolving mentions; raw IDs are shown without one.</summary>
    public object? MessageContext
    {
        get => GetValue(MessageContextProperty);
        set => SetValue(MessageContextProperty, value);
    }

    public bool IsEmojiOnly
    {
        get => (bool)GetValue(IsEmojiOnlyProperty);
        set => SetValue(IsEmojiOnlyProperty, value);
    }

    public bool IsEdited
    {
        get => (bool)GetValue(IsEditedProperty);
        set => SetValue(IsEditedProperty, value);
    }

    private static void OnChanged(DependencyObject d, DependencyPropertyChangedEventArgs e)
    {
        var control = (MarkdownTextBlock)d;
        if (control.Nodes is IReadOnlyList<MarkdownNode> nodes)
            control.Content = new Renderer(control.MessageContext as IMessageContext, control.IsEmojiOnly).Render(nodes, control.IsEdited);
        else
            control.Content = null;
    }

    private sealed class Renderer(IMessageContext? context, bool jumbo)
    {
        private const double BodyFontSize = 14;
        private const double JumboFontSize = 40;
        private double EmojiSize => jumbo ? 48 : 22;

        public UIElement Render(IReadOnlyList<MarkdownNode> nodes, bool isEdited)
        {
            var panel = new StackPanel { Spacing = 4 };
            RenderBlocks(nodes, panel.Children);
            if (isEdited)
                AppendEdited(panel);
            return panel;
        }

        private void RenderBlocks(IReadOnlyList<MarkdownNode> nodes, UIElementCollection target)
        {
            Paragraph? paragraph = null;
            foreach (var node in nodes)
            {
                if (RenderBlock(node) is { } block)
                {
                    TrimTrailingBreak(paragraph);
                    paragraph = null;
                    target.Add(block);
                    continue;
                }
                if (paragraph is null)
                {
                    var text = CreateRichText(jumbo ? JumboFontSize : BodyFontSize);
                    paragraph = new Paragraph();
                    text.Blocks.Add(paragraph);
                    target.Add(text);
                }
                AddInline(paragraph.Inlines, node, inLink: false);
            }
            TrimTrailingBreak(paragraph);
        }

        private UIElement? RenderBlock(MarkdownNode node)
        {
            switch (node)
            {
                case CodeBlockNode code:
                    return new Border
                    {
                        Background = Ui.ThemeBrush("SolidBackgroundFillColorSecondaryBrush"),
                        BorderBrush = Ui.ThemeBrush("CardStrokeColorDefaultBrush"),
                        BorderThickness = new Thickness(1),
                        CornerRadius = new CornerRadius(4),
                        Padding = new Thickness(10, 8, 10, 8),
                        Child = new ScrollViewer
                        {
                            HorizontalScrollMode = ScrollMode.Auto,
                            HorizontalScrollBarVisibility = ScrollBarVisibility.Auto,
                            VerticalScrollMode = ScrollMode.Disabled,
                            VerticalScrollBarVisibility = ScrollBarVisibility.Disabled,
                            Content = new TextBlock
                            {
                                Text = code.Code,
                                FontFamily = Ui.MonoFont,
                                FontSize = 13,
                                IsTextSelectionEnabled = true,
                            },
                        },
                    };

                case BlockQuoteNode quote:
                    var grid = new Grid { ColumnSpacing = 10 };
                    grid.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(4) });
                    grid.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
                    grid.Children.Add(new Border
                    {
                        CornerRadius = new CornerRadius(2),
                        Background = Ui.ThemeBrush("ControlStrongFillColorDisabledBrush"),
                    });
                    var inner = new StackPanel { Spacing = 4 };
                    Grid.SetColumn(inner, 1);
                    RenderBlocks(quote.Children, inner.Children);
                    grid.Children.Add(inner);
                    return grid;

                case HeadingNode heading:
                    var title = CreateRichText(heading.Level switch { 1 => 24, 2 => 20, _ => 16 });
                    title.FontWeight = Microsoft.UI.Text.FontWeights.Bold;
                    title.Margin = new Thickness(0, 4, 0, 0);
                    title.Blocks.Add(Paragraph(heading.Children));
                    return title;

                case SubtextNode subtext:
                    var small = CreateRichText(12);
                    small.Foreground = Ui.ThemeBrush("TextFillColorSecondaryBrush");
                    small.Blocks.Add(Paragraph(subtext.Children));
                    return small;

                case ListNode list:
                    var items = new StackPanel { Spacing = 2 };
                    for (var i = 0; i < list.Items.Count; i++)
                    {
                        var row = new Grid { ColumnSpacing = 6 };
                        row.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(18) });
                        row.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
                        row.Children.Add(new TextBlock
                        {
                            Text = list.Ordered ? $"{list.Start + i}." : "•",
                            HorizontalAlignment = HorizontalAlignment.Right,
                            FontSize = BodyFontSize,
                        });
                        var content = CreateRichText(BodyFontSize);
                        content.Blocks.Add(Paragraph(list.Items[i]));
                        Grid.SetColumn(content, 1);
                        row.Children.Add(content);
                        items.Children.Add(row);
                    }
                    return items;

                default:
                    return null;
            }
        }

        private Paragraph Paragraph(IReadOnlyList<MarkdownNode> nodes)
        {
            var paragraph = new Paragraph();
            foreach (var node in nodes)
                AddInline(paragraph.Inlines, node, inLink: false);
            return paragraph;
        }

        private void AddInline(InlineCollection inlines, MarkdownNode node, bool inLink)
        {
            switch (node)
            {
                case TextNode text:
                    AddText(inlines, text.Text);
                    break;
                case BoldNode bold:
                    inlines.Add(Wrap(new Bold(), bold.Children, inLink));
                    break;
                case ItalicNode italic:
                    inlines.Add(Wrap(new Italic(), italic.Children, inLink));
                    break;
                case UnderlineNode underline:
                    inlines.Add(Wrap(new Underline(), underline.Children, inLink));
                    break;
                case StrikethroughNode strike:
                    inlines.Add(Wrap(new Span { TextDecorations = Windows.UI.Text.TextDecorations.Strikethrough }, strike.Children, inLink));
                    break;
                case SpoilerNode spoiler when !inLink:
                    inlines.Add(new InlineUIContainer { Child = Spoiler(PlainText(spoiler.Children)) });
                    break;
                case ContainerNode container when inLink:
                    // Hyperlinks can't contain UI elements or other hyperlinks; flatten.
                    foreach (var child in container.Children)
                        AddInline(inlines, child, inLink);
                    break;
                case LinkNode link when !Uri.TryCreate(link.Url, UriKind.Absolute, out _):
                    foreach (var child in link.Children)
                        AddInline(inlines, child, inLink);
                    break;
                case LinkNode link when DeepLink.TryParse(link.Url, out var channelLink):
                    // Links to Discord channels open here rather than in the browser.
                    var channelHyperlink = new Hyperlink();
                    channelHyperlink.Click += (_, _) => context?.OpenLink(channelLink.ChannelId, channelLink.MessageId);
                    ToolTipService.SetToolTip(channelHyperlink, link.Url);
                    foreach (var child in link.Children)
                        AddInline(channelHyperlink.Inlines, child, inLink: true);
                    inlines.Add(channelHyperlink);
                    break;
                case LinkNode link:
                    var hyperlink = new Hyperlink { NavigateUri = new Uri(link.Url) };
                    ToolTipService.SetToolTip(hyperlink, link.Url);
                    foreach (var child in link.Children)
                        AddInline(hyperlink.Inlines, child, inLink: true);
                    inlines.Add(hyperlink);
                    break;
                case InlineCodeNode code:
                    inlines.Add(new Run
                    {
                        Text = code.Code,
                        FontFamily = Ui.MonoFont,
                        Foreground = Ui.ThemeBrush("TextFillColorSecondaryBrush"),
                    });
                    break;
                case CodeBlockNode code:
                    // Only reached inside lists/headings, which can't hold blocks.
                    inlines.Add(new Run { Text = code.Code, FontFamily = Ui.MonoFont });
                    break;
                case UserMentionNode user:
                    AddMention(inlines, "@" + (context?.UserName(user.UserId) ?? user.UserId.ToString()), 0, inLink,
                        context is null ? null : anchor => Views.ProfileCard.ShowAt(anchor, context.CreateProfile(user.UserId)));
                    break;
                case RoleMentionNode role:
                    var (roleName, color) = context?.Role(role.RoleId) ?? (role.RoleId.ToString(), 0);
                    AddMention(inlines, "@" + roleName, color, inLink);
                    break;
                case ChannelMentionNode channel:
                    AddMention(inlines, "#" + (context?.ChannelName(channel.ChannelId) ?? channel.ChannelId.ToString()), 0, inLink,
                        _ => context?.OpenChannel(channel.ChannelId));
                    break;
                case EveryoneMentionNode everyone:
                    AddMention(inlines, everyone.Here ? "@here" : "@everyone", 0, inLink);
                    break;
                case CustomEmojiNode emoji when !inLink && context is not null:
                    var image = new Image
                    {
                        Width = EmojiSize,
                        Height = EmojiSize,
                        Source = new BitmapImage(context.EmojiUri(emoji.Id, emoji.Animated, jumbo ? 96 : 48)),
                        Margin = new Thickness(1, 0, 1, jumbo ? 0 : -5),
                    };
                    ToolTipService.SetToolTip(image, $":{emoji.Name}:");
                    inlines.Add(new InlineUIContainer { Child = image });
                    break;
                case CustomEmojiNode emoji:
                    inlines.Add(new Run { Text = $":{emoji.Name}:" });
                    break;
                case TimestampNode timestamp:
                    var label = context?.FormatTimestamp(timestamp.Time, timestamp.Style) ?? timestamp.Time.ToString("F");
                    if (inLink)
                    {
                        inlines.Add(new Run { Text = label });
                    }
                    else
                    {
                        var pill = Pill(label, Ui.ThemeBrush("SubtleFillColorSecondaryBrush"), Ui.ThemeBrush("TextFillColorPrimaryBrush"));
                        ToolTipService.SetToolTip(pill, timestamp.Time.ToLocalTime().ToString("F"));
                        inlines.Add(new InlineUIContainer { Child = pill });
                    }
                    break;
                case ListNode or HeadingNode or SubtextNode or BlockQuoteNode:
                    AddText(inlines, PlainText([node]));
                    break;
            }
        }

        private Span Wrap(Span span, IReadOnlyList<MarkdownNode> children, bool inLink)
        {
            foreach (var child in children)
                AddInline(span.Inlines, child, inLink);
            return span;
        }

        private static void AddText(InlineCollection inlines, string text)
        {
            var lines = text.Split('\n');
            for (var i = 0; i < lines.Length; i++)
            {
                if (i > 0) inlines.Add(new LineBreak());
                if (lines[i].Length > 0) inlines.Add(new Run { Text = lines[i] });
            }
        }

        private static void AddMention(InlineCollection inlines, string text, int color, bool inLink, Action<FrameworkElement>? onClick = null)
        {
            if (inLink)
            {
                inlines.Add(new Run { Text = text });
                return;
            }
            var foreground = color != 0 ? Ui.Rgb(color) : Ui.ThemeBrush("AccentTextFillColorPrimaryBrush");
            var pill = Pill(text, new SolidColorBrush(Windows.UI.Color.FromArgb(0x30,
                (byte)(color != 0 ? color >> 16 : 0x58), (byte)(color != 0 ? color >> 8 : 0x65), (byte)(color != 0 ? color : 0xF2))), foreground);
            if (onClick is not null)
                pill.Tapped += (_, e) =>
                {
                    e.Handled = true;
                    onClick(pill);
                };
            inlines.Add(new InlineUIContainer { Child = pill });
        }

        private static Border Pill(string text, Brush background, Brush foreground) => new()
        {
            Background = background,
            CornerRadius = new CornerRadius(3),
            Padding = new Thickness(2, 0, 2, 1),
            Margin = new Thickness(0, 0, 0, -4),
            Child = new TextBlock
            {
                Text = text,
                Foreground = foreground,
                FontWeight = Microsoft.UI.Text.FontWeights.SemiBold,
                FontSize = BodyFontSize,
            },
        };

        private static Border Spoiler(string text)
        {
            var label = new TextBlock { Text = text, Opacity = 0, FontSize = BodyFontSize, TextWrapping = TextWrapping.Wrap };
            var border = new Border
            {
                Background = Ui.ThemeBrush("ControlStrongFillColorDefaultBrush"),
                CornerRadius = new CornerRadius(3),
                Padding = new Thickness(2, 0, 2, 1),
                Margin = new Thickness(0, 0, 0, -4),
                Child = label,
            };
            ToolTipService.SetToolTip(border, "Spoiler — click to reveal");
            border.Tapped += (_, e) =>
            {
                label.Opacity = 1;
                border.Background = Ui.ThemeBrush("SubtleFillColorSecondaryBrush");
                ToolTipService.SetToolTip(border, null);
                e.Handled = true;
            };
            return border;
        }

        private static RichTextBlock CreateRichText(double fontSize) => new()
        {
            FontSize = fontSize,
            TextWrapping = TextWrapping.Wrap,
            IsTextSelectionEnabled = true,
        };

        private static void TrimTrailingBreak(Paragraph? paragraph)
        {
            if (paragraph is not null && paragraph.Inlines.Count > 0 && paragraph.Inlines[^1] is LineBreak)
                paragraph.Inlines.RemoveAt(paragraph.Inlines.Count - 1);
        }

        private static void AppendEdited(StackPanel panel)
        {
            var edited = new Run
            {
                Text = " (edited)",
                FontSize = 11,
                Foreground = Ui.ThemeBrush("TextFillColorTertiaryBrush"),
            };
            if (panel.Children.Count > 0 && panel.Children[^1] is RichTextBlock { Blocks.Count: > 0 } last
                && last.Blocks[^1] is Paragraph paragraph)
            {
                paragraph.Inlines.Add(edited);
            }
            else
            {
                var text = new TextBlock();
                text.Inlines.Add(edited);
                panel.Children.Add(text);
            }
        }

        private static string PlainText(IReadOnlyList<MarkdownNode> nodes)
        {
            var builder = new StringBuilder();
            void Walk(MarkdownNode node)
            {
                switch (node)
                {
                    case TextNode t: builder.Append(t.Text); break;
                    case InlineCodeNode c: builder.Append(c.Code); break;
                    case CodeBlockNode c: builder.Append(c.Code); break;
                    case ContainerNode c: foreach (var child in c.Children) Walk(child); break;
                    case ListNode l: foreach (var item in l.Items) foreach (var child in item) Walk(child); break;
                    case CustomEmojiNode e: builder.Append(':').Append(e.Name).Append(':'); break;
                    case EveryoneMentionNode e: builder.Append(e.Here ? "@here" : "@everyone"); break;
                }
            }
            foreach (var node in nodes) Walk(node);
            return builder.ToString();
        }
    }
}
