using CommunityToolkit.Mvvm.ComponentModel;
using CommunityToolkit.Mvvm.Input;

namespace Swiftcord.ViewModels.Settings;

/// <summary>App preferences and updates. Outlives sign-in sessions; each change is saved immediately.</summary>
public sealed partial class SettingsViewModel : ObservableObject
{
    private readonly ISettingsStore _store;
    private readonly IUpdateService? _updates;
    private AvailableUpdate? _downloaded;

    public SettingsViewModel(ISettingsStore store, IUpdateService? updates = null)
    {
        _store = store;
        _updates = updates;
        Current = store.Load();
        _themeIndex = (int)Current.Theme;
        _notificationsEnabled = Current.NotificationsEnabled;
        _notificationPreviews = Current.NotificationPreviews;
        _automaticUpdates = Current.AutomaticUpdates;
        _updateStatus = updates is { CanUpdate: false } or null
            ? "Updates are only available in installed copies of Swiftcord."
            : "";
    }

    /// <summary>The saved settings. Replaced (not mutated) on each change, so readers on other threads see a consistent copy.</summary>
    public AppSettings Current { get; private set; }

    /// <summary>Raised on the UI thread after a setting changes and has been saved.</summary>
    public event Action<AppSettings>? Changed;

    public string Version => _updates?.CurrentVersion ?? "dev";
    public bool CanUpdate => _updates?.CanUpdate == true;

    /// <summary>Index into System/Light/Dark, for a ComboBox.</summary>
    [ObservableProperty]
    private int _themeIndex;

    [ObservableProperty]
    [NotifyPropertyChangedFor(nameof(CanChangePreviews))]
    private bool _notificationsEnabled;

    [ObservableProperty]
    private bool _notificationPreviews;

    [ObservableProperty]
    private bool _automaticUpdates;

    [ObservableProperty]
    [NotifyCanExecuteChangedFor(nameof(CheckForUpdatesCommand))]
    private bool _isCheckingForUpdates;

    [ObservableProperty]
    private string _updateStatus;

    [ObservableProperty]
    private int _updateProgress;

    [ObservableProperty]
    private bool _isUpdateReady;

    public bool CanChangePreviews => NotificationsEnabled;

    partial void OnThemeIndexChanged(int value) => Update(s => s.Theme = Enum.IsDefined((AppTheme)value) ? (AppTheme)value : AppTheme.System);
    partial void OnNotificationsEnabledChanged(bool value) => Update(s => s.NotificationsEnabled = value);
    partial void OnNotificationPreviewsChanged(bool value) => Update(s => s.NotificationPreviews = value);
    partial void OnAutomaticUpdatesChanged(bool value) => Update(s => s.AutomaticUpdates = value);

    private void Update(Action<AppSettings> change)
    {
        var next = Current.Clone();
        change(next);
        Current = next;
        try { _store.Save(next); }
        catch (Exception ex) when (ex is IOException or UnauthorizedAccessException) { /* Keep the in-memory value. */ }
        Changed?.Invoke(next);
    }

    #region Updates

    /// <summary>Checks and downloads in the background at startup, if automatic updates are on.</summary>
    public async Task CheckOnStartupAsync()
    {
        if (AutomaticUpdates)
            await CheckForUpdatesAsync();
    }

    private bool CanCheckForUpdates() => CanUpdate && !IsCheckingForUpdates;

    [RelayCommand(CanExecute = nameof(CanCheckForUpdates))]
    private async Task CheckForUpdatesAsync()
    {
        if (_updates is not { CanUpdate: true } updates || IsUpdateReady) return;
        IsCheckingForUpdates = true;
        UpdateStatus = "Checking for updates…";
        try
        {
            if (await updates.CheckAsync() is not { } update)
            {
                UpdateStatus = "Swiftcord is up to date.";
                return;
            }
            UpdateStatus = $"Downloading version {update.Version}…";
            await updates.DownloadAsync(update, new Progress<int>(p => UpdateProgress = p));
            _downloaded = update;
            IsUpdateReady = true;
            UpdateStatus = $"Version {update.Version} is ready. Restart Swiftcord to finish updating.";
        }
        catch (Exception)
        {
            UpdateStatus = "Couldn't check for updates. Try again later.";
        }
        finally
        {
            IsCheckingForUpdates = false;
        }
    }

    [RelayCommand]
    private void RestartToUpdate()
    {
        if (_downloaded is { } update)
            _updates?.ApplyAndRestart(update);
    }

    #endregion
}
