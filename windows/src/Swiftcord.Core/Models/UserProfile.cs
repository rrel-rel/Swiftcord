namespace Swiftcord.Core.Models;

/// <summary>The profile shown in a profile popup (GET /users/{id}/profile, user accounts only).</summary>
public sealed class UserProfile
{
    public User User { get; set; } = new();
    [System.Text.Json.Serialization.JsonPropertyName("user_profile")]
    public ProfileDetails? UserProfileDetails { get; set; }
    /// <summary>Per-server profile overrides (bio, pronouns, banner) when requested with a guild.</summary>
    public ProfileDetails? GuildMemberProfile { get; set; }
    public GuildMember? GuildMember { get; set; }
    public List<MutualGuild> MutualGuilds { get; set; } = [];
    public DateTimeOffset? PremiumSince { get; set; }

    /// <summary>Server-specific details win over global ones, as in the official client.</summary>
    public string? Bio => NonEmpty(GuildMemberProfile?.Bio) ?? NonEmpty(UserProfileDetails?.Bio) ?? NonEmpty(User.Bio);
    public string? Pronouns => NonEmpty(GuildMemberProfile?.Pronouns) ?? NonEmpty(UserProfileDetails?.Pronouns) ?? NonEmpty(User.Pronouns);
    public int? AccentColor => UserProfileDetails?.AccentColor ?? User.AccentColor;

    private static string? NonEmpty(string? value) => string.IsNullOrWhiteSpace(value) ? null : value;
}

public sealed class ProfileDetails
{
    public string? Bio { get; set; }
    public string? Pronouns { get; set; }
    public string? Banner { get; set; }
    public int? AccentColor { get; set; }
}

public sealed class MutualGuild
{
    public Snowflake Id { get; set; }
    public string? Nick { get; set; }
}
