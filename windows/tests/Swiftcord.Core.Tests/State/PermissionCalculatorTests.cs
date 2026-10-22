using Swiftcord.Core.Models;
using Swiftcord.Core.Permissions;

namespace Swiftcord.Core.Tests.State;

public sealed class PermissionCalculatorTests
{
    private const ulong GuildId = 10, RoleA = 11, RoleB = 12, UserId = 1;

    private static Guild Guild(ulong everyonePermissions, ulong roleAPermissions = 0) => new()
    {
        Id = GuildId,
        OwnerId = 999,
        Roles =
        [
            new Role { Id = GuildId, Permissions = everyonePermissions },
            new Role { Id = RoleA, Permissions = roleAPermissions },
            new Role { Id = RoleB },
        ],
    };

    private static PermissionOverwrite Overwrite(ulong id, OverwriteType type, Permission allow = 0, Permission deny = 0) =>
        new() { Id = id, Type = type, Allow = (ulong)allow, Deny = (ulong)deny };

    [Fact]
    public void Owner_and_administrators_have_everything()
    {
        var guild = Guild((ulong)Permission.ViewChannel);
        guild.OwnerId = UserId;
        Assert.Equal(Permission.All, PermissionCalculator.ComputeBase(guild, UserId, []));

        var admin = Guild(0, (ulong)Permission.Administrator);
        var channel = new Channel { PermissionOverwrites = [Overwrite(GuildId, OverwriteType.Role, deny: Permission.ViewChannel)] };
        Assert.Equal(Permission.All, PermissionCalculator.Compute(admin, channel, UserId, [RoleA]));
    }

    [Fact]
    public void Role_allow_beats_role_deny_and_member_overwrite_beats_both()
    {
        var guild = Guild((ulong)(Permission.ViewChannel | Permission.SendMessages));
        var channel = new Channel
        {
            PermissionOverwrites =
            [
                Overwrite(RoleA, OverwriteType.Role, deny: Permission.SendMessages),
                Overwrite(RoleB, OverwriteType.Role, allow: Permission.SendMessages),
            ],
        };

        // Role overwrites are combined: an allow from any role wins over a deny from another.
        Assert.True(PermissionCalculator.Compute(guild, channel, UserId, [RoleA, RoleB]).HasFlag(Permission.SendMessages));
        Assert.False(PermissionCalculator.Compute(guild, channel, UserId, [RoleA]).HasFlag(Permission.SendMessages));

        channel.PermissionOverwrites.Add(Overwrite(UserId, OverwriteType.Member, deny: Permission.SendMessages));
        Assert.False(PermissionCalculator.Compute(guild, channel, UserId, [RoleA, RoleB]).HasFlag(Permission.SendMessages));
    }

    [Fact]
    public void Losing_view_channel_removes_all_permissions_and_losing_send_removes_attach()
    {
        var guild = Guild((ulong)(Permission.ViewChannel | Permission.SendMessages | Permission.AttachFiles));
        var hidden = new Channel { PermissionOverwrites = [Overwrite(GuildId, OverwriteType.Role, deny: Permission.ViewChannel)] };
        Assert.Equal(Permission.None, PermissionCalculator.Compute(guild, hidden, UserId, []));

        var readOnly = new Channel { PermissionOverwrites = [Overwrite(GuildId, OverwriteType.Role, deny: Permission.SendMessages)] };
        var permissions = PermissionCalculator.Compute(guild, readOnly, UserId, []);
        Assert.True(permissions.HasFlag(Permission.ViewChannel));
        Assert.False(permissions.HasFlag(Permission.AttachFiles));
    }
}
