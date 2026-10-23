using Swiftcord.Core.Tests.Support;
using Swiftcord.ViewModels.Settings;

namespace Swiftcord.ViewModels.Tests;

public sealed class SettingsViewModelTests
{
    [Fact]
    public void Loads_saved_settings_and_saves_each_change()
    {
        var store = new MemorySettingsStore(new AppSettings { Theme = AppTheme.Dark, NotificationPreviews = false });
        var settings = new SettingsViewModel(store);
        AppSettings? changed = null;
        settings.Changed += s => changed = s;

        Assert.Equal((int)AppTheme.Dark, settings.ThemeIndex);
        Assert.False(settings.NotificationPreviews);

        settings.ThemeIndex = (int)AppTheme.Light;

        Assert.Equal(AppTheme.Light, store.Saved!.Theme);
        Assert.Equal(AppTheme.Light, changed!.Theme);
        Assert.False(changed.NotificationPreviews);
    }

    [Fact]
    public void Json_store_round_trips_and_falls_back_to_defaults()
    {
        var path = Path.Combine(Path.GetTempPath(), $"swiftcord-settings-{Guid.NewGuid():N}", "settings.json");
        var store = new JsonSettingsStore(path);
        try
        {
            Assert.True(store.Load().NotificationsEnabled);

            store.Save(new AppSettings { Theme = AppTheme.Dark, AutomaticUpdates = false });
            var loaded = store.Load();
            Assert.Equal((AppTheme.Dark, false), (loaded.Theme, loaded.AutomaticUpdates));
            Assert.Contains("\"Dark\"", File.ReadAllText(path));

            File.WriteAllText(path, "{ not json");
            Assert.Equal(AppTheme.System, store.Load().Theme);
        }
        finally
        {
            Directory.Delete(Path.GetDirectoryName(path)!, recursive: true);
        }
    }

    [Fact]
    public async Task Downloads_an_available_update_and_offers_a_restart()
    {
        var updates = new FakeUpdates { Available = new AvailableUpdate("1.2.0", new object()) };
        var settings = new SettingsViewModel(new MemorySettingsStore(), updates);

        await settings.CheckOnStartupAsync();

        Assert.True(settings.IsUpdateReady);
        // Progress<T> reports on the captured context (the thread pool here), so it lands shortly after.
        await TestUtil.EventuallyAsync(() => settings.UpdateProgress == 100);
        Assert.Contains("1.2.0", settings.UpdateStatus);

        settings.RestartToUpdateCommand.Execute(null);
        Assert.Same(updates.Available, updates.Applied);
    }

    [Fact]
    public async Task Reports_being_up_to_date_or_failing_to_check()
    {
        var updates = new FakeUpdates();
        var settings = new SettingsViewModel(new MemorySettingsStore(), updates);

        await settings.CheckForUpdatesCommand.ExecuteAsync(null);
        Assert.Equal("Swiftcord is up to date.", settings.UpdateStatus);

        updates.Fail = true;
        await settings.CheckForUpdatesCommand.ExecuteAsync(null);
        Assert.StartsWith("Couldn't check", settings.UpdateStatus);
        Assert.False(settings.IsUpdateReady);
    }

    [Fact]
    public async Task Automatic_checks_respect_the_setting_and_uninstalled_copies_cannot_update()
    {
        var updates = new FakeUpdates { Available = new AvailableUpdate("1.2.0", new object()) };
        var settings = new SettingsViewModel(new MemorySettingsStore(new AppSettings { AutomaticUpdates = false }), updates);
        await settings.CheckOnStartupAsync();
        Assert.Equal(0, updates.Checks);

        var portable = new SettingsViewModel(new MemorySettingsStore(), new FakeUpdates { CanUpdate = false });
        Assert.False(portable.CheckForUpdatesCommand.CanExecute(null));
        Assert.StartsWith("Updates are only available", portable.UpdateStatus);
    }

    private sealed class FakeUpdates : IUpdateService
    {
        public AvailableUpdate? Available { get; set; }
        public AvailableUpdate? Applied { get; private set; }
        public bool Fail { get; set; }
        public int Checks { get; private set; }
        public string CurrentVersion => "1.1.0";
        public bool CanUpdate { get; set; } = true;

        public Task<AvailableUpdate?> CheckAsync(CancellationToken ct = default)
        {
            Checks++;
            return Fail ? Task.FromException<AvailableUpdate?>(new HttpRequestException()) : Task.FromResult(Available);
        }

        public Task DownloadAsync(AvailableUpdate update, IProgress<int> progress, CancellationToken ct = default)
        {
            progress.Report(100);
            return Task.CompletedTask;
        }

        public void ApplyAndRestart(AvailableUpdate update) => Applied = update;
    }
}
