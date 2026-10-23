using Swiftcord.Core.Models;

namespace Swiftcord.Core.Permissions;

[Flags]
public enum Permission : ulong
{
    None = 0,
    CreateInstantInvite = 1UL << 0,
    KickMembers = 1UL << 1,
    BanMembers = 1UL << 2,
    Administrator = 1UL << 3,
    ManageChannels = 1UL << 4,
    ManageGuild = 1UL << 5,
    AddReactions = 1UL << 6,
    ViewAuditLog = 1UL << 7,
    PrioritySpeaker = 1UL << 8,
    Stream = 1UL << 9,
    ViewChannel = 1UL << 10,
    SendMessages = 1UL << 11,
    SendTtsMessages = 1UL << 12,
    ManageMessages = 1UL << 13,
    EmbedLinks = 1UL << 14,
    AttachFiles = 1UL << 15,
    ReadMessageHistory = 1UL << 16,
    MentionEveryone = 1UL << 17,
    UseExternalEmojis = 1UL << 18,
    ViewGuildInsights = 1UL << 19,
    Connect = 1UL << 20,
    Speak = 1UL << 21,
    MuteMembers = 1UL << 22,
    DeafenMembers = 1UL << 23,
    MoveMembers = 1UL << 24,
    UseVad = 1UL << 25,
    ChangeNickname = 1UL << 26,
    ManageNicknames = 1UL << 27,
    ManageRoles = 1UL << 28,
    ManageWebhooks = 1UL << 29,
    ManageGuildExpressions = 1UL << 30,
    UseApplicationCommands = 1UL << 31,
    RequestToSpeak = 1UL << 32,
    ManageEvents = 1UL << 33,
    ManageThreads = 1UL << 34,
    CreatePublicThreads = 1UL << 35,
    CreatePrivateThreads = 1UL << 36,
    UseExternalStickers = 1UL << 37,
    SendMessagesInThreads = 1UL << 38,
    UseEmbeddedActivities = 1UL << 39,
    ModerateMembers = 1UL << 40,
    SendVoiceMessages = 1UL << 46,
    SendPolls = 1UL << 49,
    /// <summary>Pin and unpin messages (split out of Manage Messages in 2025).</summary>
    PinMessages = 1UL << 51,
    All = ulong.MaxValue,
}

/// <summary>
/// Implements Discord's permission resolution: base guild permissions from roles, then
/// channel overwrites applied in the order @everyone → roles (combined) → member.
/// </summary>
public static class PermissionCalculator
{
    public static Permission ComputeBase(Guild guild, Snowflake userId, IReadOnlyCollection<Snowflake> memberRoles)
    {
        if (guild.OwnerId == userId)
            return Permission.All;

        var everyone = guild.Roles.FirstOrDefault(r => r.Id == guild.Id);
        var permissions = (Permission)(everyone?.Permissions ?? 0);

        foreach (var role in guild.Roles)
        {
            if (memberRoles.Contains(role.Id))
                permissions |= (Permission)role.Permissions;
        }

        return permissions.HasFlag(Permission.Administrator) ? Permission.All : permissions;
    }

    public static Permission ComputeOverwrites(
        Permission basePermissions, Channel channel, Snowflake guildId, Snowflake userId, IReadOnlyCollection<Snowflake> memberRoles)
    {
        if (basePermissions.HasFlag(Permission.Administrator))
            return Permission.All;

        var permissions = basePermissions;
        var overwrites = channel.PermissionOverwrites;

        if (overwrites.FirstOrDefault(o => o.Id == guildId) is { } everyone)
            permissions = Apply(permissions, everyone.Allow, everyone.Deny);

        ulong allow = 0, deny = 0;
        foreach (var overwrite in overwrites)
        {
            if (overwrite.Type == OverwriteType.Role && overwrite.Id != guildId && memberRoles.Contains(overwrite.Id))
            {
                allow |= overwrite.Allow;
                deny |= overwrite.Deny;
            }
        }
        permissions = Apply(permissions, allow, deny);

        if (overwrites.FirstOrDefault(o => o.Type == OverwriteType.Member && o.Id == userId) is { } member)
            permissions = Apply(permissions, member.Allow, member.Deny);

        return permissions;
    }

    /// <summary>Full permissions for a member in a guild channel.</summary>
    public static Permission Compute(Guild guild, Channel channel, Snowflake userId, IReadOnlyCollection<Snowflake> memberRoles)
    {
        var basePermissions = ComputeBase(guild, userId, memberRoles);
        var permissions = ComputeOverwrites(basePermissions, channel, guild.Id, userId, memberRoles);

        // Without ViewChannel, no other channel permission applies.
        if (!permissions.HasFlag(Permission.ViewChannel))
            return Permission.None;
        // Without SendMessages, permissions that depend on it are implicitly denied.
        if (!permissions.HasFlag(Permission.SendMessages) && !channel.IsThread)
            permissions &= ~(Permission.MentionEveryone | Permission.SendTtsMessages | Permission.AttachFiles | Permission.EmbedLinks);

        return permissions;
    }

    private static Permission Apply(Permission permissions, ulong allow, ulong deny) =>
        (permissions & ~(Permission)deny) | (Permission)allow;
}
