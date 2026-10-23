using CommunityToolkit.Mvvm.ComponentModel;
using CommunityToolkit.Mvvm.Input;
using Swiftcord.Core;
using Swiftcord.Core.Markdown;
using Swiftcord.Core.Models;
using Swiftcord.ViewModels.Formatting;

namespace Swiftcord.ViewModels.Messages;

public sealed class AttachmentViewModel
{
    /// <summary>Largest size an inline image or video preview is drawn at.</summary>
    public const double MaxPreviewWidth = 400, MaxPreviewHeight = 300;

    public AttachmentViewModel(Attachment attachment)
    {
        FileName = attachment.Filename;
        SizeText = Formatters.FileSize(attachment.Size);
        Uri = new Uri(attachment.Url);
        IsImage = attachment.IsImage && attachment.Width is > 0;
        IsVideo = attachment.IsVideo;
        IsSpoiler = attachment.IsSpoiler;
        Description = attachment.Description;

        if (IsImage)
        {
            // The media proxy resizes on the fly, so request roughly what we'll draw (at 2x for high DPI).
            (PreviewWidth, PreviewHeight) = Fit(attachment.Width!.Value, attachment.Height ?? attachment.Width.Value);
            var separator = attachment.ProxyUrl.Contains('?') ? '&' : '?';
            PreviewUri = new Uri($"{attachment.ProxyUrl}{separator}width={(int)(PreviewWidth * 2)}&height={(int)(PreviewHeight * 2)}");
        }
        else if (IsVideo && attachment.ProxyUrl.Length > 0)
        {
            IsPlayableVideo = true;
            (PreviewWidth, PreviewHeight) = Fit(attachment.Width ?? 400, attachment.Height ?? 225);
            VideoUri = new Uri(attachment.ProxyUrl);
            // The media proxy serves a video's first frame as an image when asked for one.
            var separator = attachment.ProxyUrl.Contains('?') ? '&' : '?';
            PreviewUri = new Uri($"{attachment.ProxyUrl}{separator}format=jpeg&width={(int)(PreviewWidth * 2)}&height={(int)(PreviewHeight * 2)}");
        }
    }

    public string FileName { get; }
    public string SizeText { get; }
    public string? Description { get; }
    /// <summary>The original file.</summary>
    public Uri Uri { get; }
    /// <summary>A resized image for inline display.</summary>
    public Uri? PreviewUri { get; }
    public bool IsImage { get; }
    public bool IsVideo { get; }
    /// <summary>A video that plays inline from <see cref="VideoUri"/>, with <see cref="PreviewUri"/> as its poster.</summary>
    public bool IsPlayableVideo { get; }
    public Uri? VideoUri { get; }
    public bool IsFile => !IsImage && !IsPlayableVideo;
    public bool IsSpoiler { get; }
    public double PreviewWidth { get; }
    public double PreviewHeight { get; }

    internal static (double Width, double Height) Fit(double width, double height)
    {
        if (width <= 0 || height <= 0) (width, height) = (400, 225);
        var scale = Math.Min(1, Math.Min(MaxPreviewWidth / width, MaxPreviewHeight / height));
        return (Math.Round(width * scale), Math.Round(height * scale));
    }
}

public sealed class EmbedFieldViewModel(EmbedField field, IMessageContext? context)
{
    public string Name { get; } = field.Name;
    public IReadOnlyList<MarkdownNode> Value { get; } = MarkdownParser.Parse(field.Value);
    public bool Inline { get; } = field.Inline;
    public IMessageContext? Context { get; } = context;
}

public sealed class EmbedViewModel
{
    public EmbedViewModel(Embed embed, IMessageContext? context = null)
    {
        Context = context;
        Title = embed.Title;
        Url = TryUri(embed.Url);
        Description = embed.Description is { Length: > 0 } d ? MarkdownParser.Parse(d) : [];
        Color = embed.Color;
        AuthorName = embed.Author?.Name;
        AuthorIconUri = TryUri(embed.Author?.ProxyIconUrl ?? embed.Author?.IconUrl);
        ProviderName = embed.Provider?.Name;
        FooterText = embed.Footer?.Text;
        FooterIconUri = TryUri(embed.Footer?.ProxyIconUrl ?? embed.Footer?.IconUrl);
        Fields = embed.Fields.Select(f => new EmbedFieldViewModel(f, context)).ToList();

        // Videos: Discord proxies direct video files (proxy_url), which play inline. Others (e.g. YouTube) only
        // link to a player page, so they show their thumbnail large with a play button that opens the link.
        if (embed.Video is { } video)
        {
            var size = Fit(video.Width ?? embed.Thumbnail?.Width ?? 400, video.Height ?? embed.Thumbnail?.Height ?? 225);
            (VideoWidth, VideoHeight) = size;
            VideoPosterUri = TryUri(embed.Thumbnail?.ProxyUrl ?? embed.Thumbnail?.Url);
            if (TryUri(video.ProxyUrl) is { } playable)
                VideoUri = playable;
            else
                VideoLinkUri = TryUri(embed.Url) ?? TryUri(video.Url);
        }

        // Images, GIFs and videos posted as links unfurl into a bare media embed; so do video-only embeds
        // (e.g. from link-fixing bots), which have nothing but the video.
        IsGif = embed.Type == "gifv";
        IsMediaOnly = embed.Type is "image" or "gifv"
            || (HasVideo && !HasTitle && !HasDescription && !HasAuthor && Fields.Count == 0);
        var media = embed.Image ?? (IsMediaOnly && !HasVideo ? embed.Thumbnail : null);
        if (media is not null)
        {
            ImageUri = TryUri(media.ProxyUrl ?? media.Url);
            (ImageWidth, ImageHeight) = AttachmentViewModel.Fit(media.Width ?? 400, media.Height ?? 225);
        }
        else if (embed.Thumbnail is { } thumbnail && !HasVideo)
        {
            ThumbnailUri = TryUri(thumbnail.ProxyUrl ?? thumbnail.Url);
        }
    }

    private static (double, double) Fit(double width, double height) => AttachmentViewModel.Fit(width, height);

    public IMessageContext? Context { get; }
    public string? Title { get; }
    public Uri? Url { get; }
    public IReadOnlyList<MarkdownNode> Description { get; }
    public int? Color { get; }
    /// <summary>The side-bar colour as plain RGB, 0 for none; x:Bind functions handle non-nullable args best.</summary>
    public int AccentColor => Color ?? 0;
    public string? AuthorName { get; }
    public Uri? AuthorIconUri { get; }
    public string? ProviderName { get; }
    public string? FooterText { get; }
    public Uri? FooterIconUri { get; }
    public IReadOnlyList<EmbedFieldViewModel> Fields { get; }
    public Uri? ImageUri { get; }
    public double ImageWidth { get; }
    public double ImageHeight { get; }
    public Uri? ThumbnailUri { get; }
    public bool IsMediaOnly { get; }
    /// <summary>A GIF (Tenor, Giphy…), which Discord delivers as a silent looping video.</summary>
    public bool IsGif { get; }
    /// <summary>A directly playable video file.</summary>
    public Uri? VideoUri { get; }
    /// <summary>A video that can't play inline; clicking its poster opens this page.</summary>
    public Uri? VideoLinkUri { get; }
    public Uri? VideoPosterUri { get; }
    public double VideoWidth { get; }
    public double VideoHeight { get; }
    public bool HasVideo => VideoUri is not null || VideoLinkUri is not null;
    public bool HasPlayableVideo => VideoUri is not null;
    public bool HasVideoLink => VideoUri is null && VideoLinkUri is not null;
    public bool ShowsImage => HasImage && !HasVideo;

    public bool HasTitle => !string.IsNullOrEmpty(Title);
    public bool HasDescription => Description.Count > 0;
    public bool HasAuthor => !string.IsNullOrEmpty(AuthorName);
    public bool HasProvider => !string.IsNullOrEmpty(ProviderName);
    public bool HasFooter => !string.IsNullOrEmpty(FooterText);
    public bool HasImage => ImageUri is not null;
    public bool HasThumbnail => ThumbnailUri is not null;
    public bool HasFields => Fields.Count > 0;

    /// <summary>Embeds with nothing to show (e.g. a link whose preview failed) are hidden.</summary>
    public bool IsEmpty => !HasTitle && !HasDescription && !HasAuthor && !HasImage && !HasThumbnail && !HasFields && !HasFooter && !HasVideo;

    private static Uri? TryUri(string? url) =>
        Uri.TryCreate(url, UriKind.Absolute, out var uri) && uri.Scheme is "http" or "https" ? uri : null;
}

public sealed class StickerViewModel(StickerItem sticker)
{
    public string Name { get; } = sticker.Name;
    /// <summary>Null for Lottie stickers, which can't be drawn as an image.</summary>
    public Uri? ImageUri { get; } = sticker.FormatType == StickerFormat.Lottie ? null : new Uri(Cdn.Sticker(sticker)!);
    public bool HasImage => ImageUri is not null;
}

public sealed partial class ReactionViewModel : ObservableObject
{
    public ReactionViewModel(Reaction reaction, Func<ReactionViewModel, Task> toggle)
    {
        Emoji = reaction.Emoji;
        _count = reaction.Count;
        _me = reaction.Me;
        ImageUri = reaction.Emoji.Id is { } id ? new Uri(Cdn.Emoji(id, reaction.Emoji.Animated, 32)) : null;
        ToggleCommand = new AsyncRelayCommand(() => toggle(this));
    }

    public Emoji Emoji { get; }
    /// <summary>The unicode emoji; empty for custom emoji, which use <see cref="ImageUri"/>.</summary>
    public string Text => Emoji.Id is null ? Emoji.Name ?? "" : "";
    public string Name => Emoji.Name ?? "";
    public Uri? ImageUri { get; }
    public bool IsCustom => ImageUri is not null;
    public IAsyncRelayCommand ToggleCommand { get; }

    [ObservableProperty]
    private int _count;

    [ObservableProperty]
    private bool _me;
}

/// <summary>A file chosen in the composer but not yet sent.</summary>
public sealed class PendingAttachment(string fileName, long size, Func<Task<Stream>> open, string? contentType = null)
{
    public string FileName { get; } = fileName;
    public string SizeText { get; } = Formatters.FileSize(size);
    public long Size { get; } = size;
    public string? ContentType { get; } = contentType;
    public bool IsSpoiler { get; set; }

    internal Func<Task<Stream>> Open { get; } = open;

    /// <summary>Set by the chat view model so the composer chip can remove itself.</summary>
    public IRelayCommand? RemoveCommand { get; internal set; }
}
