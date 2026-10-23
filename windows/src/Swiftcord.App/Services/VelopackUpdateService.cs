using Swiftcord.ViewModels.Settings;
using Velopack;
using Velopack.Sources;

namespace Swiftcord.App.Services;

/// <summary>Updates from the GitHub releases that Velopack's <c>vpk upload github</c> publishes.</summary>
public sealed class VelopackUpdateService : IUpdateService
{
    public const string RepositoryUrl = "https://github.com/rrel-rel/Swiftcord";

    private readonly UpdateManager _manager = new(new GithubSource(RepositoryUrl, null, prerelease: false));

    public string CurrentVersion => _manager.CurrentVersion?.ToString()
        ?? typeof(VelopackUpdateService).Assembly.GetName().Version?.ToString(3) ?? "dev";

    public bool CanUpdate => _manager.IsInstalled;

    public async Task<AvailableUpdate?> CheckAsync(CancellationToken ct = default)
    {
        var info = await _manager.CheckForUpdatesAsync().WaitAsync(ct);
        return info is null ? null : new AvailableUpdate(info.TargetFullRelease.Version.ToString(), info);
    }

    public Task DownloadAsync(AvailableUpdate update, IProgress<int> progress, CancellationToken ct = default) =>
        _manager.DownloadUpdatesAsync((UpdateInfo)update.Handle, progress.Report, ct);

    public void ApplyAndRestart(AvailableUpdate update) =>
        _manager.ApplyUpdatesAndRestart(((UpdateInfo)update.Handle).TargetFullRelease);
}
