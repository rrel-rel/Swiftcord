using System.Text.Json;
using Swiftcord.Core.Json;
using Swiftcord.Core.Models;
using Swiftcord.ViewModels.Messages;

namespace Swiftcord.ViewModels.Tests;

public sealed class MediaTests
{
    private static EmbedViewModel Embed(string json) => new(DiscordJson.Deserialize<Embed>(JsonDocument.Parse(json).RootElement));

    [Fact]
    public void A_video_only_embed_plays_inline_instead_of_being_hidden()
    {
        // What link-fixing bots (e.g. for Instagram reels) produce: a video and nothing else.
        var embed = Embed("""
            {"type":"video","url":"https://ddinstagram.com/reel/x",
             "video":{"url":"https://cdn/x.mp4","proxy_url":"https://images-ext-1.discordapp.net/x.mp4","width":720,"height":1280}}
            """);

        Assert.False(embed.IsEmpty);
        Assert.True(embed.IsMediaOnly);
        Assert.True(embed.HasPlayableVideo);
        Assert.Equal("https://images-ext-1.discordapp.net/x.mp4", embed.VideoUri!.AbsoluteUri);
        Assert.Equal(300, embed.VideoHeight); // Fitted into the preview box.
        Assert.False(embed.ShowsImage);
    }

    [Fact]
    public void A_youtube_embed_keeps_its_card_and_links_its_large_thumbnail_to_the_page()
    {
        var embed = Embed("""
            {"type":"video","url":"https://www.youtube.com/watch?v=x","title":"A video","provider":{"name":"YouTube"},
             "thumbnail":{"url":"https://i.ytimg.com/x.jpg","proxy_url":"https://images-ext-1.discordapp.net/x.jpg","width":1280,"height":720},
             "video":{"url":"https://www.youtube.com/embed/x","width":1280,"height":720}}
            """);

        Assert.False(embed.IsMediaOnly);
        Assert.True(embed.HasVideoLink);
        Assert.False(embed.HasPlayableVideo);
        Assert.Equal("https://www.youtube.com/watch?v=x", embed.VideoLinkUri!.AbsoluteUri);
        Assert.Equal("https://images-ext-1.discordapp.net/x.jpg", embed.VideoPosterUri!.AbsoluteUri);
        Assert.False(embed.HasThumbnail); // Shown as the poster rather than the small corner thumbnail.
    }

    [Fact]
    public void Gif_embeds_loop_their_video()
    {
        var embed = Embed("""
            {"type":"gifv","url":"https://tenor.com/x","provider":{"name":"Tenor"},
             "thumbnail":{"url":"https://media.tenor.com/x.png","proxy_url":"https://p/x.png","width":498,"height":280},
             "video":{"url":"https://media.tenor.com/x.mp4","proxy_url":"https://p/x.mp4","width":498,"height":280}}
            """);

        Assert.True(embed.IsGif);
        Assert.True(embed.IsMediaOnly);
        Assert.Equal("https://p/x.mp4", embed.VideoUri!.AbsoluteUri);
    }

    [Fact]
    public void Image_embeds_are_unchanged()
    {
        var embed = Embed("""{"type":"image","url":"https://x/y.png","thumbnail":{"url":"https://x/y.png","proxy_url":"https://p/y.png","width":100,"height":50}}""");

        Assert.True(embed.IsMediaOnly);
        Assert.True(embed.ShowsImage);
        Assert.False(embed.HasVideo);
        Assert.Equal((100.0, 50.0), (embed.ImageWidth, embed.ImageHeight));
    }

    [Fact]
    public void Video_attachments_play_inline_with_a_first_frame_poster()
    {
        var attachment = new AttachmentViewModel(new Attachment
        {
            Filename = "clip.mp4", ContentType = "video/mp4", Size = 1000, Width = 1920, Height = 1080,
            Url = "https://cdn.discordapp.com/attachments/1/2/clip.mp4?ex=1",
            ProxyUrl = "https://media.discordapp.net/attachments/1/2/clip.mp4?ex=1",
        });

        Assert.True(attachment.IsPlayableVideo);
        Assert.False(attachment.IsFile);
        Assert.Equal("https://media.discordapp.net/attachments/1/2/clip.mp4?ex=1", attachment.VideoUri!.AbsoluteUri);
        Assert.Contains("&format=jpeg&width=800&height=450", attachment.PreviewUri!.AbsoluteUri);
    }
}
