using System.Collections.ObjectModel;
using CommunityToolkit.Mvvm.ComponentModel;
using Swiftcord.Core;
using Swiftcord.Core.Models;

namespace Swiftcord.ViewModels.Members;

/// <summary>A group header in the member list, e.g. "ONLINE — 12" or a hoisted role.</summary>
public sealed record MemberGroupRow(string Title);

/// <summary>A member in the member list.</summary>
public sealed record MemberRow(Snowflake UserId, string Name, int Color, Uri AvatarUri, PresenceStatus Status, string? ActivityText)
{
    public bool IsOffline => Status is PresenceStatus.Offline or PresenceStatus.Invisible;
    public bool HasActivity => !string.IsNullOrEmpty(ActivityText);
}

/// <summary>A row Discord hasn't sent yet (beyond the subscribed range).</summary>
public sealed record MemberPlaceholderRow;

/// <summary>
/// The member sidebar for a server channel. Subscribes to the channel's member list (op 14, user accounts) and
/// mirrors Discord's index-based SYNC/INSERT/UPDATE/DELETE/INVALIDATE ops onto <see cref="Rows"/>.
/// </summary>
public sealed partial class MemberListViewModel : ObservableObject, IDisposable
{
    /// <summary>Discord serves the list in windows of 100 rows.</summary>
    public const int RangeSize = 100;

    private readonly DiscordClient _client;
    private readonly IUiDispatcher _ui;
    private readonly Snowflake _guildId;
    private readonly Snowflake _channelId;
    private string _listId;
    private bool _received;
    private int _requestedEnd = RangeSize - 1;
    private bool _disposed;

    public MemberListViewModel(DiscordClient client, IUiDispatcher ui, Channel channel)
    {
        _client = client;
        _ui = ui;
        _guildId = channel.GuildId ?? throw new ArgumentException("Member lists belong to server channels.", nameof(channel));
        _channelId = channel.Id;
        // Threads share their parent channel's list.
        var source = channel.IsThread && channel.ParentId is { } parent && client.State.GetChannel(parent) is { } p ? p : channel;
        _listId = MemberListId.For(source);
        client.State.MemberListUpdated += OnUpdate;
    }

    public ObservableCollection<object> Rows { get; } = [];

    [ObservableProperty]
    private bool _isLoading = true;

    [ObservableProperty]
    private string _summary = "";

    /// <summary>Subscribes to the first window of the list.</summary>
    public Task StartAsync() => SubscribeAsync();

    /// <summary>Call as rows come into view; asks for the next window when nearing the end of what's loaded.</summary>
    public void EnsureLoaded(int index)
    {
        if (index < _requestedEnd - RangeSize / 2 || _requestedEnd >= TotalRows - 1)
            return;
        _requestedEnd += RangeSize;
        _ = SubscribeAsync();
    }

    private int TotalRows { get; set; } = RangeSize;

    private async Task SubscribeAsync()
    {
        // Always keep the first window; add the window ending at the furthest row asked for (max three ranges).
        var ranges = new List<(int, int)> { (0, RangeSize - 1) };
        if (_requestedEnd >= RangeSize)
            ranges.Add((_requestedEnd - RangeSize + 1, _requestedEnd));
        try
        {
            await _client.Gateway.SubscribeGuildAsync(_guildId, new Dictionary<Snowflake, (int, int)[]> { [_channelId] = [.. ranges] });
        }
        catch (Exception)
        {
            // Not connected; the list will be requested again when the channel reopens.
        }
    }

    private void OnUpdate(MemberListUpdate update)
    {
        if (update.GuildId != _guildId) return;
        _ui.Post(() =>
        {
            if (_disposed) return;
            if (update.Id != _listId)
            {
                // If the computed ID ever disagrees with Discord's, follow the first list that arrives.
                if (_received) return;
                _listId = update.Id;
            }
            _received = true;
            Apply(update);
        });
    }

    private void Apply(MemberListUpdate update)
    {
        TotalRows = update.Groups.Sum(g => g.Count + 1);
        Summary = update.OnlineCount > 0 ? $"{update.OnlineCount:N0} online · {update.MemberCount:N0} members" : $"{update.MemberCount:N0} members";
        var groups = update.Groups.ToDictionary(g => g.Id, g => g.Count);

        foreach (var op in update.Ops)
        {
            switch (op.Op)
            {
                case "SYNC" when op.Range is [var start, _] && op.Items is { } items:
                    for (var i = 0; i < items.Count; i++)
                        Set(start + i, ToRow(items[i], groups));
                    break;
                case "INSERT" when op is { Index: { } index, Item: { } item }:
                    Pad(index);
                    Rows.Insert(index, ToRow(item, groups));
                    break;
                case "UPDATE" when op is { Index: { } index, Item: { } item }:
                    Set(index, ToRow(item, groups));
                    break;
                case "DELETE" when op.Index is { } index && index < Rows.Count:
                    Rows.RemoveAt(index);
                    break;
                case "INVALIDATE" when op.Range is [var start, var end]:
                    for (var i = start; i <= end && i < Rows.Count; i++)
                        Rows[i] = new MemberPlaceholderRow();
                    break;
            }
        }
        while (Rows.Count > TotalRows)
            Rows.RemoveAt(Rows.Count - 1);
        IsLoading = false;
    }

    private void Set(int index, object row)
    {
        Pad(index);
        if (index < Rows.Count)
            Rows[index] = row;
        else
            Rows.Add(row);
    }

    private void Pad(int index)
    {
        while (Rows.Count < index)
            Rows.Add(new MemberPlaceholderRow());
    }

    private object ToRow(MemberListItem item, Dictionary<string, int> counts)
    {
        if (item.Group is { } group)
        {
            var count = counts.GetValueOrDefault(group.Id, group.Count);
            var name = group.Id switch
            {
                "online" => "Online",
                "offline" => "Offline",
                _ => Snowflake.TryParse(group.Id, out var roleId)
                    ? _client.State.GetGuild(_guildId)?.Roles.Find(r => r.Id == roleId)?.Name ?? "Role"
                    : group.Id,
            };
            return new MemberGroupRow($"{name.ToUpperInvariant()} — {count:N0}");
        }

        if (item.Member is not { } member)
            return new MemberPlaceholderRow();
        var user = member.User;
        var status = PresenceStatusExtensions.Parse(member.Presence?.Status);
        return new MemberRow(
            user.Id,
            member.Nick ?? user.DisplayName,
            _client.State.GetRoleColor(_guildId, user.Id) ?? 0,
            new Uri(Cdn.MemberAvatar(_guildId, user, member.ToGuildMember(), 64)),
            status,
            ActivityText(member.Presence?.Activities ?? []));
    }

    /// <summary>The line under a member's name: their custom status, or what they're doing.</summary>
    public static string? ActivityText(IReadOnlyList<Activity> activities)
    {
        if (activities.FirstOrDefault(a => a.Type == 4) is { } custom)
        {
            // Unicode emoji can go in the text; custom (image) emoji can't, so they're left out.
            var emoji = custom.Emoji is { Id: null, Name: { } e } ? e + " " : "";
            var text = (emoji + custom.State).Trim();
            if (text.Length > 0) return text;
        }
        return activities.FirstOrDefault(a => a.Type != 4) switch
        {
            { Type: 0 } a => $"Playing {a.Name}",
            { Type: 1 } a => $"Streaming {a.Details ?? a.Name}",
            { Type: 2 } a => $"Listening to {a.Name}",
            { Type: 3 } a => $"Watching {a.Name}",
            { Type: 5 } a => $"Competing in {a.Name}",
            _ => null,
        };
    }

    public void Dispose()
    {
        if (_disposed) return;
        _disposed = true;
        _client.State.MemberListUpdated -= OnUpdate;
    }
}
