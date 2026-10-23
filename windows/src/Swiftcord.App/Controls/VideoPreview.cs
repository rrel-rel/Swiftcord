using Microsoft.Extensions.Logging;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Media;
using Microsoft.UI.Xaml.Media.Imaging;
using Swiftcord.App.Helpers;
using Windows.Media.Core;
using Windows.Media.Playback;
using Windows.System;

namespace Swiftcord.App.Controls;

/// <summary>
/// An inline video. Shows a poster with a play button and only creates a player once clicked, so a long
/// message list doesn't hold dozens of players. GIFs play straight away, muted and looping, like Discord.
/// Without a <see cref="Source"/> (e.g. YouTube), clicking opens <see cref="LinkUri"/> instead.
/// </summary>
public sealed partial class VideoPreview : ContentControl
{
    public static readonly DependencyProperty SourceProperty = DependencyProperty.Register(
        nameof(Source), typeof(Uri), typeof(VideoPreview), new PropertyMetadata(null, OnChanged));

    public static readonly DependencyProperty PosterProperty = DependencyProperty.Register(
        nameof(Poster), typeof(Uri), typeof(VideoPreview), new PropertyMetadata(null, OnChanged));

    public static readonly DependencyProperty LinkUriProperty = DependencyProperty.Register(
        nameof(LinkUri), typeof(Uri), typeof(VideoPreview), new PropertyMetadata(null, OnChanged));

    public static readonly DependencyProperty IsGifProperty = DependencyProperty.Register(
        nameof(IsGif), typeof(bool), typeof(VideoPreview), new PropertyMetadata(false, OnChanged));

    /// <summary>GIFs only start once on screen this long, so fast scrolling doesn't churn through players.</summary>
    private static readonly TimeSpan GifStartDelay = TimeSpan.FromMilliseconds(400);

    private MediaPlayer? _player;
    private int _version;

    public VideoPreview()
    {
        IsTabStop = false;
        HorizontalContentAlignment = HorizontalAlignment.Stretch;
        VerticalContentAlignment = VerticalAlignment.Stretch;
        CornerRadius = new CornerRadius(6);
        Loaded += (_, _) => Rebuild();
        // Leaving the screen (or the list recycling this row) stops and releases the player.
        Unloaded += (_, _) =>
        {
            _version++; // Cancels a pending GIF start.
            ReleasePlayer();
        };
    }

    public Uri? Source
    {
        get => (Uri?)GetValue(SourceProperty);
        set => SetValue(SourceProperty, value);
    }

    public Uri? Poster
    {
        get => (Uri?)GetValue(PosterProperty);
        set => SetValue(PosterProperty, value);
    }

    public Uri? LinkUri
    {
        get => (Uri?)GetValue(LinkUriProperty);
        set => SetValue(LinkUriProperty, value);
    }

    public bool IsGif
    {
        get => (bool)GetValue(IsGifProperty);
        set => SetValue(IsGifProperty, value);
    }

    private static void OnChanged(DependencyObject d, DependencyPropertyChangedEventArgs e) => ((VideoPreview)d).Rebuild();

    private async void Rebuild()
    {
        var version = ++_version;
        ReleasePlayer();
        ShowPoster();
        if (!IsGif || Source is null) return;
        await Task.Delay(GifStartDelay);
        if (version == _version && IsLoaded)
            Play(muted: true, loop: true, controls: false);
    }

    private void ShowPoster()
    {
        var grid = new Grid { Background = Ui.ThemeBrush("SolidBackgroundFillColorTertiaryBrush") };
        if (Poster is { } poster)
            grid.Children.Add(new Image { Source = new BitmapImage(poster), Stretch = Stretch.UniformToFill });

        var play = new Button
        {
            Width = 56,
            Height = 56,
            HorizontalAlignment = HorizontalAlignment.Center,
            VerticalAlignment = VerticalAlignment.Center,
            CornerRadius = new CornerRadius(28),
            Background = new SolidColorBrush(Windows.UI.Color.FromArgb(0xB0, 0, 0, 0)),
            BorderThickness = new Thickness(0),
            Content = new FontIcon { Glyph = Source is null ? "" : "", FontSize = 22, Foreground = new SolidColorBrush(Microsoft.UI.Colors.White) },
        };
        ToolTipService.SetToolTip(play, Source is null ? "Open video" : "Play");
        play.Click += async (_, _) =>
        {
            if (Source is not null)
                Play(muted: false, loop: false, controls: true);
            else if (LinkUri is { } link)
                await Launcher.LaunchUriAsync(link);
        };
        grid.Children.Add(play);
        Content = grid;
    }

    private void Play(bool muted, bool loop, bool controls)
    {
        if (Source is not { } source) return;
        _player = new MediaPlayer
        {
            Source = MediaSource.CreateFromUri(source),
            IsMuted = muted,
            IsLoopingEnabled = loop,
            AutoPlay = true,
        };
        var element = new MediaPlayerElement
        {
            AreTransportControlsEnabled = controls,
            Stretch = Stretch.Uniform,
        };
        _player.MediaFailed += (_, e) =>
            App.Instance?.CreateLogger<VideoPreview>().LogWarning("Video failed ({Error}): {Message} {Source}", e.Error, e.ErrorMessage, source);
        element.SetMediaPlayer(_player);
        Content = element;
    }

    private void ReleasePlayer()
    {
        if (_player is null) return;
        if (Content is MediaPlayerElement element)
            element.SetMediaPlayer(null);
        _player.Dispose();
        _player = null;
        Content = null;
    }
}
