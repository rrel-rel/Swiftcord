namespace Swiftcord.ViewModels.Settings;

/// <summary>An available update, as found by <see cref="IUpdateService.CheckAsync"/>.</summary>
public sealed record AvailableUpdate(string Version, object Handle);

/// <summary>Checks for, downloads and applies app updates (Velopack on Windows).</summary>
public interface IUpdateService
{
    string CurrentVersion { get; }

    /// <summary>False when running from a build folder rather than an installed copy; updates are unavailable then.</summary>
    bool CanUpdate { get; }

    Task<AvailableUpdate?> CheckAsync(CancellationToken ct = default);

    Task DownloadAsync(AvailableUpdate update, IProgress<int> progress, CancellationToken ct = default);

    /// <summary>Exits, installs the downloaded update and restarts.</summary>
    void ApplyAndRestart(AvailableUpdate update);
}
