using CommunityToolkit.Mvvm.ComponentModel;
using CommunityToolkit.Mvvm.Input;
using Swiftcord.Core;
using Swiftcord.Core.Markdown;
using Swiftcord.Core.Models;
using Swiftcord.ViewModels.Messages;

namespace Swiftcord.ViewModels.Profiles;

public sealed record RoleChip(string Name, int Color);

/// <summary>
/// A profile popup. Shows what the cache knows straight away, then fills in the bio, pronouns, banner and
/// server join date from the profile endpoint (user accounts only).
/// </summary>
public sealed partial class ProfileViewModel : ObservableObject
{
    private readonly DiscordClient _client;
    private readonly Snowflake _userId;
    private readonly Snowflake? _guildId;
    private readonly Action<Snowflake>? _openChannel;

    public ProfileViewModel(DiscordClient client, Snowflake userId, Snowflake? guildId, IMessageContext? context = null,
        Action<Snowflake>? openChannel = null)
    {
        _client = client;
        _userId = userId;
        _guildId = guildId;
        _openChannel = openChannel;
        Context = context;

        var state = client.State;
        var user = state.GetUser(userId) ?? new User { Id = userId, Username = "unknown" };
        var member = guildId is { } g ? state.GetMember(g, userId) : null;
        IsSelf = state.CurrentUser?.Id == userId;
        IsBot = user.Bot;
        Username = user.Tag;
        DisplayName = state.GetDisplayName(userId, guildId);
        AvatarUri = new Uri(guildId is { } gid ? Cdn.MemberAvatar(gid, user, member, 160) : Cdn.Avatar(user, 160));
        AccountCreated = userId.CreatedAt.ToLocalTime().ToString("MMM d, yyyy");
        Status = state.GetPresence(userId);
        Apply(user, member, profile: null, guildDetails: null);
    }

    public IMessageContext? Context { get; }
    public bool IsSelf { get; }
    public bool IsBot { get; }
    public string Username { get; }
    public string DisplayName { get; }
    public Uri AvatarUri { get; }
    public string AccountCreated { get; }
    public PresenceStatus Status { get; }
    public bool CanMessage => !IsSelf;

    [ObservableProperty]
    [NotifyPropertyChangedFor(nameof(HasBanner))]
    private Uri? _bannerUri;

    /// <summary>The banner colour when there's no banner image (RGB, 0 for the default).</summary>
    [ObservableProperty]
    private int _bannerColor;

    [ObservableProperty]
    [NotifyPropertyChangedFor(nameof(HasPronouns))]
    private string? _pronouns;

    [ObservableProperty]
    [NotifyPropertyChangedFor(nameof(HasBio))]
    private IReadOnlyList<MarkdownNode> _bio = [];

    [ObservableProperty]
    [NotifyPropertyChangedFor(nameof(HasServerJoined))]
    private string? _serverJoined;

    [ObservableProperty]
    [NotifyPropertyChangedFor(nameof(HasRoles))]
    private IReadOnlyList<RoleChip> _roles = [];

    [ObservableProperty]
    private string? _mutualServers;

    [ObservableProperty]
    private string? _error;

    public bool HasBanner => BannerUri is not null;
    public bool HasPronouns => !string.IsNullOrEmpty(Pronouns);
    public bool HasBio => Bio.Count > 0;
    public bool HasServerJoined => ServerJoined is not null;
    public bool HasRoles => Roles.Count > 0;

    /// <summary>Fetches the full profile. Failure (e.g. for bot tokens) leaves the cached details in place.</summary>
    public async Task LoadAsync()
    {
        if (_client.Options.TokenType != TokenType.User)
            return;
        try
        {
            var profile = await _client.Rest.GetUserProfileAsync(_userId, _guildId);
            Apply(profile.User, profile.GuildMember, profile, profile.GuildMemberProfile);
            if (!IsSelf)
                MutualServers = profile.MutualGuilds.Count switch
                {
                    0 => null,
                    1 => "1 mutual server",
                    var n => $"{n} mutual servers",
                };
        }
        catch (Exception)
        {
            // Profiles of users who share no server with us aren't available; the cached view stands.
        }
    }

    private void Apply(User user, GuildMember? member, UserProfile? profile, ProfileDetails? guildDetails)
    {
        BannerUri = (_guildId is { } g && Cdn.MemberBanner(g, _userId, guildDetails?.Banner) is { } memberBanner ? new Uri(memberBanner) : null)
            ?? (Cdn.UserBanner(_userId, profile?.UserProfileDetails?.Banner ?? user.Banner) is { } banner ? new Uri(banner) : null);
        BannerColor = profile?.AccentColor ?? user.AccentColor ?? 0;
        Pronouns = profile?.Pronouns ?? user.Pronouns;
        var bio = profile?.Bio ?? user.Bio;
        Bio = string.IsNullOrWhiteSpace(bio) ? [] : MarkdownParser.Parse(bio);

        if (_guildId is { } guildId && member is not null)
        {
            if (member.JoinedAt is { } joined)
                ServerJoined = joined.ToLocalTime().ToString("MMM d, yyyy");
            var guild = _client.State.GetGuild(guildId);
            Roles = guild is null ? [] : guild.Roles
                .Where(r => member.Roles.Contains(r.Id))
                .OrderByDescending(r => r.Position)
                .Select(r => new RoleChip(r.Name, r.Color))
                .ToList();
        }
    }

    [RelayCommand]
    private async Task SendMessageAsync()
    {
        try
        {
            var channel = await _client.OpenDmAsync(_userId);
            _openChannel?.Invoke(channel.Id);
        }
        catch (Exception)
        {
            Error = "Couldn't open a conversation with this user.";
        }
    }
}
