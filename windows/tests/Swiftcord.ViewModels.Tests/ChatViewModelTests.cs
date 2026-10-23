using System.Net;
using Microsoft.Extensions.Time.Testing;
using Swiftcord.Core;
using Swiftcord.ViewModels.Messages;
using static Swiftcord.ViewModels.Tests.TestSession;
using Support = Swiftcord.Core.Tests.Support.TestUtil;

namespace Swiftcord.ViewModels.Tests;

public sealed class ChatViewModelTests : IAsyncLifetime
{
    private readonly TestSession _session = new TestSession().WithReady();
    private readonly FakeTimeProvider _time = new(DateTimeOffset.Parse("2024-05-01T13:00:00Z"));
    private ChatViewModel _chat = null!;

    public Task InitializeAsync() => Task.CompletedTask;

    public async Task DisposeAsync()
    {
        _chat?.Dispose();
        await _session.DisposeAsync();
    }

    private async Task<ChatViewModel> OpenAsync(ulong channelId = General, params string[] history)
    {
        _session.Respond = (request, _) => request.Method == HttpMethod.Get
            ? Json($"[{string.Join(",", history.Reverse())}]") // REST returns newest first.
            : Json("{}");
        _chat = new ChatViewModel(_session.Client, ImmediateUiDispatcher.Instance, _session.Client.State.GetChannel(channelId)!, time: _time);
        await _chat.InitializeAsync();
        return _chat;
    }

    [Fact]
    public async Task Loads_history_oldest_first_and_groups_consecutive_messages()
    {
        var chat = await OpenAsync(General,
            MessageJson(41, General, Friend, "one", "2024-05-01T12:00:00+00:00"),
            MessageJson(42, General, Friend, "two", "2024-05-01T12:05:00+00:00"),
            MessageJson(43, General, Friend, "three", "2024-05-01T12:20:00+00:00"),
            MessageJson(44, General, Me, "four", "2024-05-01T12:21:00+00:00"));

        Assert.Equal(["one", "two", "three", "four"], chat.Messages.Select(m => m.Content));
        Assert.Equal([true, false, true, true], chat.Messages.Select(m => m.IsGroupStart));
        Assert.True(chat.Messages[0].ShowDateSeparator);
        Assert.False(chat.Messages[1].ShowDateSeparator);
        Assert.Equal("Message #general", chat.Placeholder);
        Assert.True(chat.CanSend);
        Assert.Contains("GET /api/v10/channels/100/messages?limit=50", _session.RequestLines);
    }

    [Fact]
    public async Task Opening_an_unread_channel_acknowledges_it()
    {
        await OpenAsync(General);
        Assert.Contains("POST /api/v10/channels/100/messages/50/ack", _session.RequestLines);
        Assert.False(_session.Client.State.IsUnread(General));
    }

    [Fact]
    public async Task Sending_shows_a_pending_message_until_the_server_confirms()
    {
        var chat = await OpenAsync(General, MessageJson(41, General, Friend));
        var release = new TaskCompletionSource();
        string? sentBody = null;
        _session.Respond = (request, body) =>
        {
            if (request.Method != HttpMethod.Post || !request.RequestUri!.AbsolutePath.EndsWith("/messages"))
                return Json("{}");
            sentBody = body;
            release.Task.Wait();
            var nonce = System.Text.Json.JsonDocument.Parse(body!).RootElement.GetProperty("nonce").GetString();
            return Json(MessageJson(99, General, Me, "hello world", extra: $$""","nonce":"{{nonce}}" """));
        };

        chat.Draft = "  hello world ";
        // The fake handler blocks synchronously, so send from another thread.
        var send = Task.Run(() => chat.SendCommand.ExecuteAsync(null));
        await Support.EventuallyAsync(() => sentBody is not null);

        var pending = chat.Messages.Last();
        Assert.True(pending.IsPending);
        Assert.Equal("hello world", pending.Content);
        Assert.Equal("", chat.Draft);

        release.SetResult();
        await send;
        var confirmed = chat.Messages.Last();
        Assert.False(confirmed.IsPending);
        Assert.Equal(99UL, confirmed.Id.Value);
        Assert.Equal(2, chat.Messages.Count);
        Assert.Contains("\"content\":\"hello world\"", sentBody);
    }

    [Fact]
    public async Task Gateway_echo_before_the_rest_response_does_not_duplicate_the_message()
    {
        var chat = await OpenAsync(General);
        string? nonce = null;
        _session.Respond = (request, body) =>
        {
            if (request.Method != HttpMethod.Post || !request.RequestUri!.AbsolutePath.EndsWith("/messages"))
                return Json("{}");
            nonce = System.Text.Json.JsonDocument.Parse(body!).RootElement.GetProperty("nonce").GetString();
            // The gateway event lands while the HTTP request is still in flight.
            _session.Dispatch("MESSAGE_CREATE", MessageJson(99, General, Me, "yo", extra: $$""","nonce":"{{nonce}}" """));
            return Json(MessageJson(99, General, Me, "yo"));
        };

        chat.Draft = "yo";
        await chat.SendCommand.ExecuteAsync(null);

        var message = Assert.Single(chat.Messages);
        Assert.Equal(99UL, message.Id.Value);
        Assert.False(message.IsPending);
    }

    [Fact]
    public async Task Failed_sends_are_marked_and_can_be_retried()
    {
        var chat = await OpenAsync(General);
        var fail = true;
        _session.Respond = (request, _) =>
            request.Method != HttpMethod.Post || !request.RequestUri!.AbsolutePath.EndsWith("/messages") ? Json("{}")
            : fail ? Json("""{"code":50035,"message":"Invalid Form Body"}""", HttpStatusCode.BadRequest)
            : Json(MessageJson(99, General, Me, "retry me"));

        chat.Draft = "retry me";
        await chat.SendCommand.ExecuteAsync(null);
        var failed = Assert.Single(chat.Messages);
        Assert.True(failed.IsFailed);
        Assert.Contains("Invalid Form Body", chat.Error);

        fail = false;
        await failed.RetryCommand.ExecuteAsync(null);
        var sent = Assert.Single(chat.Messages);
        Assert.False(sent.IsFailed);
        Assert.Equal(99UL, sent.Id.Value);
    }

    [Fact]
    public async Task Up_arrow_edits_the_last_own_message_and_sending_patches_it()
    {
        var chat = await OpenAsync(General, MessageJson(41, General, Me, "typo"), MessageJson(42, General, Friend, "reply"));

        Assert.True(chat.EditLastOwnMessage());
        Assert.True(chat.IsEditing);
        Assert.Equal("typo", chat.Draft);

        chat.Draft = "fixed";
        await chat.SendCommand.ExecuteAsync(null);

        Assert.Contains("PATCH /api/v10/channels/100/messages/41", _session.RequestLines);
        Assert.False(chat.IsEditing);
        Assert.Equal("", chat.Draft);
    }

    [Fact]
    public async Task Replies_send_a_message_reference_and_clear_afterwards()
    {
        var chat = await OpenAsync(General, MessageJson(41, General, Friend, "question"));
        chat.Messages[0].ReplyCommand.Execute(null);
        Assert.True(chat.IsReplying);

        chat.Draft = "answer";
        await chat.SendCommand.ExecuteAsync(null);

        var body = _session.Http.Requests.Last(r => r.Request.Method == HttpMethod.Post && r.Request.RequestUri!.AbsolutePath.EndsWith("/messages")).Body!;
        Assert.Contains("\"message_reference\":{\"type\":0,\"message_id\":\"41\"", body);
        Assert.False(chat.IsReplying);
    }

    [Fact]
    public async Task Gateway_events_update_delete_and_regroup_messages()
    {
        var chat = await OpenAsync(General,
            MessageJson(41, General, Friend, "a", "2024-05-01T12:00:00+00:00"),
            MessageJson(42, General, Me, "b", "2024-05-01T12:01:00+00:00"),
            MessageJson(43, General, Friend, "c", "2024-05-01T12:02:00+00:00"));
        Assert.True(chat.Messages[2].IsGroupStart);

        _session.Dispatch("MESSAGE_UPDATE", $$"""{"id":"41","channel_id":"{{General}}","content":"a (edited)","edited_timestamp":"2024-05-01T12:03:00+00:00"}""");
        Assert.Equal("a (edited)", chat.Messages[0].Content);
        Assert.True(chat.Messages[0].IsEdited);

        _session.Dispatch("MESSAGE_DELETE", $$"""{"id":"42","channel_id":"{{General}}"}""");
        Assert.Equal(["a (edited)", "c"], chat.Messages.Select(m => m.Content));
        Assert.False(chat.Messages[1].IsGroupStart); // Now directly follows the same author.

        _session.Dispatch("MESSAGE_CREATE", MessageJson(44, RandomChannel, Friend, "elsewhere"));
        Assert.Equal(2, chat.Messages.Count);
    }

    [Fact]
    public async Task Typing_indicator_names_typists_and_expires()
    {
        var chat = await OpenAsync(General);
        _session.Dispatch("TYPING_START", $$"""{"channel_id":"{{General}}","user_id":"{{Friend}}","timestamp":0}""");
        _session.Dispatch("TYPING_START", $$"""{"channel_id":"{{General}}","user_id":"3","timestamp":0}""");

        Assert.Equal("Friend and Unknown User are typing…", chat.TypingText);
    }

    [Fact]
    public async Task Typing_is_sent_at_most_every_eight_seconds()
    {
        var chat = await OpenAsync(General);
        chat.Draft = "h";
        chat.Draft = "he";
        _time.Advance(TimeSpan.FromSeconds(9));
        chat.Draft = "hey";
        await Task.Delay(50);

        Assert.Equal(2, _session.RequestLines.Count(l => l == "POST /api/v10/channels/100/typing"));
    }

    [Fact]
    public async Task Toggling_a_reaction_is_optimistic_and_calls_the_api()
    {
        var chat = await OpenAsync(General, MessageJson(41, General, Friend, extra: ""","reactions":[{"count":1,"me":false,"emoji":{"id":null,"name":"🔥"}}]"""));
        var reaction = chat.Messages[0].Reactions.Single();

        await reaction.ToggleCommand.ExecuteAsync(null);

        Assert.True(reaction.Me);
        Assert.Equal(2, reaction.Count);
        Assert.Contains("PUT /api/v10/channels/100/messages/41/reactions/%F0%9F%94%A5/@me", _session.RequestLines);

        await chat.Messages[0].AddReactionCommand.ExecuteAsync("👍");
        Assert.Equal(["🔥", "👍"], chat.Messages[0].Reactions.Select(r => r.Text));
    }

    [Fact]
    public async Task Load_older_prepends_history_and_stops_at_the_beginning()
    {
        var chat = await OpenAsync(General, MessageJson(41, General, Friend, "newest"));
        _session.Respond = (_, _) => Json($"[{MessageJson(30, General, Friend, "older")}]");

        await chat.LoadOlderCommand.ExecuteAsync(null);

        Assert.Equal(["older", "newest"], chat.Messages.Select(m => m.Content));
        Assert.Contains("GET /api/v10/channels/100/messages?limit=50&before=41", _session.RequestLines);
        Assert.False(chat.Messages[1].IsGroupStart);
        // A short page isn't proof of the beginning; keep going until Discord returns nothing.
        Assert.True(chat.HasMoreHistory);

        _session.Respond = (_, _) => Json("[]");
        await chat.LoadOlderCommand.ExecuteAsync(null);
        Assert.Contains("GET /api/v10/channels/100/messages?limit=50&before=30", _session.RequestLines);
        Assert.False(chat.HasMoreHistory);

        var requests = _session.Http.Requests.Count;
        await chat.LoadOlderCommand.ExecuteAsync(null);
        Assert.Equal(requests, _session.Http.Requests.Count);
    }

    [Fact]
    public async Task Jumping_to_a_loaded_message_highlights_it_briefly()
    {
        var chat = await OpenAsync(General, MessageJson(41, General, Friend, "one"), MessageJson(42, General, Friend, "two"));
        var requests = _session.Http.Requests.Count;

        await chat.JumpToMessageAsync(new Snowflake(41));

        Assert.Equal(requests, _session.Http.Requests.Count);
        Assert.Same(chat.Messages[0], chat.Highlighted);
        Assert.True(chat.Messages[0].IsHighlighted);
        _time.Advance(TimeSpan.FromSeconds(3));
        await Support.EventuallyAsync(() => !chat.Messages[0].IsHighlighted);
    }

    [Fact]
    public async Task Jumping_to_old_history_detaches_from_the_present_until_caught_up()
    {
        var chat = await OpenAsync(General, MessageJson(50, General, Friend, "latest"));
        _session.Respond = (_, _) => Json($"[{MessageJson(11, General, Friend, "after")},{MessageJson(10, General, Friend, "target")}]");

        await chat.JumpToMessageAsync(new Snowflake(10));

        Assert.Contains("GET /api/v10/channels/100/messages?limit=50&around=10", _session.RequestLines);
        Assert.Equal(["target", "after"], chat.Messages.Select(m => m.Content));
        Assert.Equal("target", chat.Highlighted!.Content);
        Assert.True(chat.IsViewingHistory);
        // Old pages stay out of the cache, which must remain contiguous with the latest message.
        Assert.Null(_session.Client.State.GetMessage(new Snowflake(General), new Snowflake(10)));

        // New messages don't land after unrelated history.
        _session.Dispatch("MESSAGE_CREATE", MessageJson(60, General, Friend, "new"));
        Assert.DoesNotContain(chat.Messages, m => m.Content == "new");

        // Paging forward with a short page reaches the present.
        _session.Respond = (_, _) => Json($"[{MessageJson(60, General, Friend, "new")},{MessageJson(50, General, Friend, "latest")}]");
        await chat.LoadNewerCommand.ExecuteAsync(null);
        Assert.Contains("GET /api/v10/channels/100/messages?limit=50&after=11", _session.RequestLines);
        Assert.Equal(["target", "after", "latest", "new"], chat.Messages.Select(m => m.Content));
        Assert.False(chat.IsViewingHistory);
    }

    [Fact]
    public async Task Jump_to_present_reloads_the_latest_messages()
    {
        var chat = await OpenAsync(General, MessageJson(50, General, Friend, "latest"));
        _session.Respond = (_, _) => Json($"[{MessageJson(10, General, Friend, "old")}]");
        await chat.JumpToMessageAsync(new Snowflake(10));
        Assert.True(chat.IsViewingHistory);

        await chat.JumpToPresentCommand.ExecuteAsync(null);

        Assert.False(chat.IsViewingHistory);
        Assert.Equal(["latest"], chat.Messages.Select(m => m.Content));
    }

    [Fact]
    public async Task A_missing_message_reports_an_error()
    {
        var chat = await OpenAsync(General, MessageJson(50, General, Friend, "latest"));
        _session.Respond = (_, _) => Json("[]");

        await chat.JumpToMessageAsync(new Snowflake(10));

        Assert.StartsWith("Couldn't find that message", chat.Error);
        Assert.Equal(["latest"], chat.Messages.Select(m => m.Content));
    }

    [Fact]
    public async Task Clicking_a_reply_jumps_to_the_original()
    {
        var chat = await OpenAsync(General,
            MessageJson(41, General, Friend, "original"),
            MessageJson(42, General, Me, "reply", extra: """
                ,"type":19,"message_reference":{"message_id":"41"},
                "referenced_message":{"id":"41","channel_id":"100","author":{"id":"2","username":"u2"},"content":"original","timestamp":"2024-05-01T12:00:00+00:00","type":0}
                """));

        await chat.Messages[1].JumpToReplyCommand.ExecuteAsync(null);

        Assert.Same(chat.Messages[0], chat.Highlighted);
    }

    [Fact]
    public async Task Inaccessible_channels_show_an_error()
    {
        _session.Respond = (_, _) => Json("""{"code":50001,"message":"Missing Access"}""", HttpStatusCode.Forbidden);
        _chat = new ChatViewModel(_session.Client, ImmediateUiDispatcher.Instance, _session.Client.State.GetChannel(General)!);
        await _chat.InitializeAsync();

        Assert.Equal("You don't have access to this channel.", _chat.Error);
        Assert.False(_chat.IsLoading);
    }

    [Fact]
    public async Task Markdown_context_resolves_names()
    {
        var chat = await OpenAsync(Dm);
        Assert.Equal("Friend", chat.UserName(Friend));
        Assert.Equal("general", chat.ChannelName(General));
        Assert.Equal("Message @Friend", chat.Placeholder);
        Assert.True(chat.IsDirectMessage);
    }
}
