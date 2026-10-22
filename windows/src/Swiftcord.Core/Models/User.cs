namespace Swiftcord.Core.Models;

public sealed class User
{
    public Snowflake Id { get; set; }
    public string Username { get; set; } = "";
    /// <summary>"0" for accounts migrated to unique usernames.</summary>
    public string Discriminator { get; set; } = "0";
    public string? GlobalName { get; set; }
    public string? Avatar { get; set; }
    public string? Banner { get; set; }
    public int? AccentColor { get; set; }
    public bool Bot { get; set; }
    public bool System { get; set; }
    public int PublicFlags { get; set; }
    public string? Pronouns { get; set; }
    public string? Bio { get; set; }

    public string DisplayName => string.IsNullOrEmpty(GlobalName) ? Username : GlobalName;

    /// <summary>Username with the legacy #discriminator suffix when the account still has one.</summary>
    public string Tag => Discriminator is "0" or "" ? Username : $"{Username}#{Discriminator}";
}

public enum PresenceStatus
{
    Offline,
    Online,
    Idle,
    DoNotDisturb,
    Invisible,
}

public static class PresenceStatusExtensions
{
    public static PresenceStatus Parse(string? value) => value switch
    {
        "online" => PresenceStatus.Online,
        "idle" => PresenceStatus.Idle,
        "dnd" => PresenceStatus.DoNotDisturb,
        "invisible" => PresenceStatus.Invisible,
        _ => PresenceStatus.Offline,
    };

    public static string ToWire(this PresenceStatus status) => status switch
    {
        PresenceStatus.Online => "online",
        PresenceStatus.Idle => "idle",
        PresenceStatus.DoNotDisturb => "dnd",
        PresenceStatus.Invisible => "invisible",
        _ => "offline",
    };
}
