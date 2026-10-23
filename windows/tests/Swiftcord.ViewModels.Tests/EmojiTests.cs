using Swiftcord.Core;
using Swiftcord.ViewModels.Emojis;
using static Swiftcord.ViewModels.Tests.TestSession;

namespace Swiftcord.ViewModels.Tests;

public sealed class EmojiTests : IAsyncLifetime
{
    private readonly TestSession _session = new TestSession().WithReady();

    public Task InitializeAsync()
    {
        _session.Dispatch("GUILD_EMOJIS_UPDATE", $$"""
            {"guild_id":"{{GuildId}}","emojis":[{"id":"900","name":"blobwave","animated":true},{"id":"901","name":"gone","available":false}]}
            """);
        _session.Dispatch("GUILD_CREATE", """{"id":"20","name":"Elsewhere","emojis":[{"id":"902","name":"blobcat"}]}""");
        return Task.CompletedTask;
    }

    public async Task DisposeAsync() => await _session.DisposeAsync();

    [Fact]
    public void The_catalog_uses_discord_names_and_fully_qualified_sequences()
    {
        Assert.Equal("😂", EmojiCatalog.ByName("joy")!.Text);
        Assert.Equal("❤️", EmojiCatalog.ByName("heart")!.Text); // Text-default: needs FE0F.
        Assert.Equal("👍", EmojiCatalog.ByName("thumbsup")!.Text);        // Emoji-default: no FE0F.
        Assert.Equal(5, EmojiCatalog.ByName("thumbsup")!.Skins!.Count);
        Assert.DoesNotContain(EmojiCatalog.Groups, g => g.Contains("Component", StringComparison.OrdinalIgnoreCase));
        Assert.True(EmojiCatalog.All.Count > 1800);
    }

    [Fact]
    public void Search_ranks_shortcode_prefixes_first()
    {
        var results = EmojiCatalog.Search(":smile").Take(3).Select(e => e.Name).ToList();
        Assert.Equal("smile", results[0]);
        Assert.Contains(EmojiCatalog.Search("pizza"), e => e.Text == "🍕");
    }

    [Fact]
    public void Picker_lists_this_server_first_and_locks_others_without_nitro()
    {
        var picker = new EmojiPickerViewModel(_session.Client.State, new Snowflake(GuildId));
        // "Recently used" (shared across pickers) may come first; then this server.
        var sections = picker.Sections.Where(s => s.Title != "Recently used").ToList();

        Assert.Equal("Swift Community", sections[0].Title);
        var wave = Assert.Single(sections[0]);       // Unavailable emoji are left out.
        Assert.Equal("<a:blobwave:900>", wave.MessageText);
        Assert.False(wave.IsLocked);
        var elsewhere = picker.Sections.Single(s => s.Title == "Elsewhere").Single();
        Assert.True(elsewhere.IsLocked);
        Assert.False(picker.Pick(elsewhere));
        Assert.Equal("Smileys & Emotion", sections[2].Title);
    }

    [Fact]
    public void Searching_mixes_custom_and_unicode_and_picks_are_remembered()
    {
        var picker = new EmojiPickerViewModel(_session.Client.State, new Snowflake(GuildId));
        EmojiItem? picked = null;
        picker.Picked += e => picked = e;

        picker.Query = "blob";
        Assert.Equal(["blobwave", "blobcat"], picker.Results.Select(r => r.Name));
        Assert.Equal("blobwave", picker.FirstResult!.Name);

        picker.Query = "joy";
        Assert.True(picker.Pick(picker.FirstResult!));
        Assert.Equal("😂", picked!.MessageText);

        var next = new EmojiPickerViewModel(_session.Client.State, null);
        Assert.Equal("Recently used", next.Sections[0].Title);
        Assert.Equal("joy", next.Sections[0][0].Name);
    }

    [Fact]
    public async Task Custom_emoji_reactions_use_the_name_and_id_route()
    {
        _session.Respond = (request, _) => request.Method == HttpMethod.Get
            ? Json($"[{MessageJson(41, General, Friend)}]")
            : Json("{}");
        using var chat = new Messages.ChatViewModel(_session.Client, ImmediateUiDispatcher.Instance, _session.Client.State.GetChannel(new Snowflake(General))!);
        await chat.InitializeAsync();

        var wave = new EmojiPickerViewModel(_session.Client.State, new Snowflake(GuildId)).Sections.Single(s => s.Title == "Swift Community")[0];
        await chat.AddReactionAsync(chat.Messages[0], wave.Emoji);

        Assert.Contains($"PUT /api/v10/channels/{General}/messages/41/reactions/blobwave%3A900/@me", _session.RequestLines);
        Assert.Equal(900UL, chat.Messages[0].Reactions.Single().Emoji.Id!.Value.Value);
    }
}
