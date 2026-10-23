using System.Text.Json;
using System.Text.Json.Serialization;

namespace Swiftcord.ViewModels.Settings;

public enum AppTheme
{
    System,
    Light,
    Dark,
}

/// <summary>Preferences that belong to this app install rather than the Discord account.</summary>
public sealed class AppSettings
{
    public AppTheme Theme { get; set; } = AppTheme.System;
    public bool NotificationsEnabled { get; set; } = true;
    /// <summary>Show message text in notifications; off shows only who sent it and where.</summary>
    public bool NotificationPreviews { get; set; } = true;
    public bool AutomaticUpdates { get; set; } = true;

    public AppSettings Clone() => (AppSettings)MemberwiseClone();
}

public interface ISettingsStore
{
    AppSettings Load();
    void Save(AppSettings settings);
}

/// <summary>Keeps settings in a JSON file, falling back to defaults if it's missing or unreadable.</summary>
public sealed class JsonSettingsStore(string path) : ISettingsStore
{
    private static readonly JsonSerializerOptions Options = new()
    {
        WriteIndented = true,
        Converters = { new JsonStringEnumConverter() },
    };

    public AppSettings Load()
    {
        try
        {
            using var stream = File.OpenRead(path);
            return JsonSerializer.Deserialize<AppSettings>(stream, Options) ?? new AppSettings();
        }
        catch (Exception ex) when (ex is IOException or UnauthorizedAccessException or JsonException)
        {
            return new AppSettings();
        }
    }

    public void Save(AppSettings settings)
    {
        Directory.CreateDirectory(Path.GetDirectoryName(Path.GetFullPath(path))!);
        // Write then swap so a crash mid-write can't leave a truncated file.
        var temp = path + ".tmp";
        File.WriteAllText(temp, JsonSerializer.Serialize(settings, Options));
        File.Move(temp, path, overwrite: true);
    }
}
