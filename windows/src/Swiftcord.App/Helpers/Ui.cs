using Microsoft.UI;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Media;
using Swiftcord.Core.Models;
using Windows.UI;

namespace Swiftcord.App.Helpers;

/// <summary>Conversion functions called from x:Bind expressions.</summary>
/// <remarks>
/// Functions bound to Visibility must return Visibility, not bool: the XAML compiler's implicit
/// bool→Visibility cast for function results generates code that doesn't compile (CS0103 'obj').
/// </remarks>
public static class Ui
{
    private static readonly Dictionary<int, SolidColorBrush> RgbBrushes = [];
    public static readonly FontFamily SymbolFont = new("Segoe Fluent Icons,Segoe MDL2 Assets");
    public static readonly FontFamily TextFont = new("Segoe UI Variable Text,Segoe UI");
    public static readonly FontFamily MonoFont = new("Cascadia Mono,Consolas");

    /// <summary>The window content's actual theme; set by MainWindow, which rebuilds its content when it changes.</summary>
    public static ElementTheme Theme { get; set; } = ElementTheme.Light;

    /// <summary>
    /// A theme brush for <see cref="Theme"/>. Application.Resources[key] would resolve against the app-wide
    /// theme, which follows Windows and ignores the theme chosen in Settings.
    /// </summary>
    public static Brush ThemeBrush(string key)
    {
        // WinUI's own dictionaries key the dark theme as "Default".
        string[] themeKeys = Theme == ElementTheme.Dark ? ["Dark", "Default"] : ["Light"];
        foreach (var themeKey in themeKeys)
        {
            if (FindInThemeDictionaries(Application.Current.Resources, themeKey, key) is Brush brush)
                return brush;
        }
        return (Brush)Application.Current.Resources[key];
    }

    private static object? FindInThemeDictionaries(ResourceDictionary dictionary, string themeKey, string key)
    {
        if (dictionary.ThemeDictionaries.TryGetValue(themeKey, out var theme) && theme is ResourceDictionary themed
            && themed.TryGetValue(key, out var value))
            return value;
        // Later merged dictionaries take precedence, as in XAML lookup.
        for (var i = dictionary.MergedDictionaries.Count - 1; i >= 0; i--)
        {
            if (FindInThemeDictionaries(dictionary.MergedDictionaries[i], themeKey, key) is { } found)
                return found;
        }
        return null;
    }

    public static Visibility Visible(bool value) => value ? Visibility.Visible : Visibility.Collapsed;

    public static Visibility Visible2(bool first, bool second) => Visible(first && second);

    public static Visibility Collapsed(bool value) => value ? Visibility.Collapsed : Visibility.Visible;

    public static Visibility VisibleIfText(string? value) => Visible(!string.IsNullOrWhiteSpace(value));

    public static bool HasText(string? value) => !string.IsNullOrWhiteSpace(value);

    public static bool Not(bool value) => !value;

    public static SolidColorBrush Rgb(int rgb)
    {
        if (!RgbBrushes.TryGetValue(rgb, out var brush))
            RgbBrushes[rgb] = brush = new SolidColorBrush(Color.FromArgb(255, (byte)(rgb >> 16), (byte)(rgb >> 8), (byte)rgb));
        return brush;
    }

    /// <summary>A role colour, or the default text colour for uncoloured roles.</summary>
    public static Brush RoleBrush(int rgb) => rgb == 0 ? ThemeBrush("TextFillColorPrimaryBrush") : Rgb(rgb);

    public static Brush EmbedBorder(int rgb) => rgb > 0 ? Rgb(rgb) : ThemeBrush("ControlStrongStrokeColorDefaultBrush");

    // ── Server rail ──

    /// <summary>Icons morph from circles to rounded squares when selected, like Discord.</summary>
    public static CornerRadius GuildCorner(bool selected) => new(selected ? 16 : 24);

    public static double PillHeight(bool selected, bool unread) => selected ? 36 : unread ? 8 : 0;

    /// <summary>The translucent folder colour behind a folder and its servers; transparent outside folders (-1).</summary>
    public static Brush FolderTint(int rgb) => rgb < 0
        ? new SolidColorBrush(Colors.Transparent)
        : new SolidColorBrush(Color.FromArgb(0x40, (byte)(rgb >> 16), (byte)(rgb >> 8), (byte)rgb));

    /// <summary>The folder background is rounded at the bottom under its last server.</summary>
    public static CornerRadius FolderBottom(bool last) => last ? new CornerRadius(0, 0, 16, 16) : new CornerRadius(0);

    // ── Channel sidebar ──

    public static Brush SelectionBrush(bool selected) =>
        selected ? ThemeBrush("SubtleFillColorSecondaryBrush") : new SolidColorBrush(Colors.Transparent);

    public static Brush ChannelForeground(bool unread, bool selected) =>
        ThemeBrush(unread || selected ? "TextFillColorPrimaryBrush" : "TextFillColorSecondaryBrush");

    // Fully qualified: FontWeights exists in both Microsoft.UI.Text and Windows.UI.Text.
    public static Windows.UI.Text.FontWeight UnreadWeight(bool unread) =>
        unread ? Microsoft.UI.Text.FontWeights.SemiBold : Microsoft.UI.Text.FontWeights.Normal;

    public static FontFamily GlyphFont(bool symbol) => symbol ? SymbolFont : TextFont;

    public static string ChevronGlyph(bool collapsed) => collapsed ? "\uE76C" : "\uE70D";

    public static Brush PresenceBrush(PresenceStatus status) => status switch
    {
        PresenceStatus.Online => Rgb(0x23A55A),
        PresenceStatus.Idle => Rgb(0xF0B232),
        PresenceStatus.DoNotDisturb => Rgb(0xF23F43),
        _ => Rgb(0x80848E),
    };

    public static Visibility PresenceVisibility(PresenceStatus status) =>
        Visible(status is not (PresenceStatus.Offline or PresenceStatus.Invisible));

    // ── Messages ──

    public static Thickness GroupMargin(bool groupStart) => new(0, groupStart ? 12 : 0, 0, 0);

    public static Brush MessageBackground(bool mentionsMe, bool highlighted) =>
        highlighted ? new SolidColorBrush(Color.FromArgb(0x33, 0x58, 0x65, 0xF2))
        : mentionsMe ? new SolidColorBrush(Color.FromArgb(0x1A, 0xF0, 0xB2, 0x32))
        : new SolidColorBrush(Colors.Transparent);

    public static Thickness MentionBorder(bool mentionsMe) => new(mentionsMe ? 2 : 0, 0, 0, 0);

    public static Thickness ForwardBar(bool forwarded) => new(forwarded ? 3 : 0, 0, 0, 0);

    public static Thickness ForwardPadding(bool forwarded) => new(forwarded ? 10 : 0, 0, 0, 0);

    public static double PendingOpacity(bool pending) => pending ? 0.5 : 1;

    public static string ChannelHeaderGlyph(bool isDirectMessage) => isDirectMessage ? "@" : "#";
}
