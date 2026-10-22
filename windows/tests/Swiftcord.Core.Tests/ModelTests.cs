using Swiftcord.Core.Json;
using Swiftcord.Core.Models;
using static Swiftcord.Core.Tests.Support.TestUtil;

namespace Swiftcord.Core.Tests;

public sealed class ModelTests
{
    [Fact]
    public void Snowflakes_encode_their_creation_time()
    {
        Snowflake id = 175928847299117063;
        Assert.Equal(DateTimeOffset.Parse("2016-04-30T11:18:25.796Z"), id.CreatedAt);
        Assert.True(Snowflake.FromTime(id.CreatedAt) <= id);
    }

    [Fact]
    public void Deserializes_messages_with_unknown_fields_and_string_or_number_ids()
    {
        var message = DiscordJson.Deserialize<Message>(ParseJson("""
            {"id":"1","channel_id":2,"author":{"id":"3","username":"a","global_name":"Alice"},
             "content":"x","timestamp":"2024-01-01T00:00:00.000000+00:00","type":19,"flags":4,
             "brand_new_field":{"nested":true},
             "attachments":[{"id":"9","filename":"SPOILER_cat.png","size":10,"url":"u","proxy_url":"p","content_type":"image/png"}],
             "referenced_message":{"id":"0","channel_id":"2","author":{"id":"4","username":"b"},"content":"orig","timestamp":"2024-01-01T00:00:00+00:00","type":0}}
            """));

        Assert.Equal(2UL, message.ChannelId.Value);
        Assert.Equal(MessageType.Reply, message.Type);
        Assert.True(message.Flags.HasFlag(MessageFlags.SuppressEmbeds));
        Assert.Equal("Alice", message.Author.DisplayName);
        Assert.True(message.Attachments[0].IsImage);
        Assert.True(message.Attachments[0].IsSpoiler);
        Assert.Equal("orig", message.ReferencedMessage!.Content);
    }

    [Fact]
    public void Permission_bitfields_round_trip_as_strings()
    {
        var role = DiscordJson.Deserialize<Role>(ParseJson("""{"id":"1","name":"r","permissions":"1099511627775"}"""));
        Assert.Equal(1099511627775UL, role.Permissions);
        Assert.Contains("\"permissions\":\"1099511627775\"", System.Text.Encoding.UTF8.GetString(DiscordJson.SerializeToUtf8Bytes(role)));
    }

    [Theory]
    [InlineData("0", 175928847299117063UL, "https://cdn.discordapp.com/embed/avatars/2.png")]
    [InlineData("1337", 1UL, "https://cdn.discordapp.com/embed/avatars/2.png")]
    public void Default_avatars_follow_discords_indexing(string discriminator, ulong id, string expected) =>
        Assert.Equal(expected, Cdn.DefaultAvatar(new User { Id = id, Discriminator = discriminator }));

    [Fact]
    public void Animated_avatars_use_gif_only_when_requested()
    {
        var user = new User { Id = 5, Avatar = "a_abc" };
        Assert.Equal("https://cdn.discordapp.com/avatars/5/a_abc.webp?size=64", Cdn.Avatar(user, 64));
        Assert.Equal("https://cdn.discordapp.com/avatars/5/a_abc.gif?size=64", Cdn.Avatar(user, 64, animate: true));
    }
}
