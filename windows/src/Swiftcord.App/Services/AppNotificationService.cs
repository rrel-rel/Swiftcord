using Microsoft.Extensions.Logging;
using Microsoft.Windows.AppNotifications;
using Microsoft.Windows.AppNotifications.Builder;
using Swiftcord.Core;
using Swiftcord.ViewModels.Navigation;
using Swiftcord.ViewModels.Notifications;

namespace Swiftcord.App.Services;

/// <summary>
/// Shows message notifications in the Windows notification centre. Clicking one raises
/// <see cref="Invoked"/> with a link to the conversation (or, if the app had closed, relaunches it with that link).
/// </summary>
public sealed class AppNotificationService : INotificationSink, IDisposable
{
    private static readonly HttpClient Http = new() { Timeout = TimeSpan.FromSeconds(5) };

    private readonly ILogger _log;
    private readonly string _avatarCache;
    private readonly bool _registered;

    public AppNotificationService(ILogger<AppNotificationService> log)
    {
        _log = log;
        _avatarCache = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "Swiftcord", "Cache", "Avatars");
        try
        {
            if (!AppNotificationManager.IsSupported())
                return;
            // Subscribe before registering, or clicks that arrive during startup are lost.
            AppNotificationManager.Default.NotificationInvoked += OnInvoked;
            AppNotificationManager.Default.Register();
            _registered = true;
        }
        catch (Exception ex)
        {
            _log.LogWarning(ex, "Notifications are unavailable");
        }
    }

    /// <summary>Raised on a background thread when the user clicks a notification.</summary>
    public event Action<DeepLink>? Invoked;

    /// <summary>The link carried by a notification's arguments, for clicks that launched the app.</summary>
    public static DeepLink? LinkFrom(AppNotificationActivatedEventArgs args) =>
        args.Arguments.TryGetValue("link", out var link) && DeepLink.TryParse(link, out var parsed) ? parsed : null;

    public void Show(MessageNotification notification) => _ = ShowAsync(notification);

    private async Task ShowAsync(MessageNotification notification)
    {
        if (!_registered) return;
        try
        {
            var link = new DeepLink(notification.GuildId, notification.ChannelId, notification.MessageId);
            var builder = new AppNotificationBuilder()
                .AddArgument("link", link.ToUri().ToString())
                .AddText(notification.Title, new AppNotificationTextProperties().SetMaxLines(1))
                .AddText(notification.Body);
            if (notification.AvatarUrl is { } url && await CacheAvatarAsync(url) is { } avatar)
                builder.SetAppLogoOverride(avatar, AppNotificationImageCrop.Circle);

            var toast = builder.BuildNotification();
            toast.Group = notification.Group;
            toast.Tag = notification.MessageId.ToString();
            AppNotificationManager.Default.Show(toast);
        }
        catch (Exception ex)
        {
            _log.LogWarning(ex, "Couldn't show a notification");
        }
    }

    public void Clear(Snowflake channelId)
    {
        if (!_registered) return;
        _ = AppNotificationManager.Default.RemoveByGroupAsync(channelId.ToString()).AsTask()
            .ContinueWith(t => _log.LogDebug(t.Exception, "Couldn't clear notifications"), TaskContinuationOptions.OnlyOnFaulted);
    }

    /// <summary>Notifications for unpackaged apps can only show local images, so avatars are downloaded first.</summary>
    private async Task<Uri?> CacheAvatarAsync(Uri url)
    {
        var name = Convert.ToHexString(System.Security.Cryptography.SHA1.HashData(System.Text.Encoding.UTF8.GetBytes(url.AbsoluteUri))) + ".png";
        var path = Path.Combine(_avatarCache, name);
        try
        {
            if (!File.Exists(path))
            {
                Directory.CreateDirectory(_avatarCache);
                var bytes = await Http.GetByteArrayAsync(url);
                await File.WriteAllBytesAsync(path, bytes);
            }
            return new Uri(path);
        }
        catch (Exception ex) when (ex is HttpRequestException or TaskCanceledException or IOException)
        {
            return null;
        }
    }

    private void OnInvoked(AppNotificationManager sender, AppNotificationActivatedEventArgs args)
    {
        if (LinkFrom(args) is { } link)
            Invoked?.Invoke(link);
    }

    public void Dispose()
    {
        if (!_registered) return;
        AppNotificationManager.Default.NotificationInvoked -= OnInvoked;
        AppNotificationManager.Default.Unregister();
    }
}
