using Microsoft.Extensions.Logging;
using Microsoft.UI.Xaml;
using Microsoft.Windows.AppLifecycle;
using Microsoft.Windows.AppNotifications;
using Swiftcord.App.Services;
using Swiftcord.Core;
using Swiftcord.Core.Auth;
using Swiftcord.ViewModels.Login;
using Swiftcord.ViewModels.Navigation;
using Swiftcord.ViewModels.Settings;
using Windows.ApplicationModel.Activation;

namespace Swiftcord.App;

public partial class App : Application
{
    private const string BotTokenPrefix = "Bot ";

    private readonly ITokenStore _tokens = new CredentialTokenStore();
    private readonly ILoggerFactory _loggerFactory = LoggerFactory.Create(b => b
        .AddDebug()
        .AddProvider(new FileLoggerProvider())
        .SetMinimumLevel(LogLevel.Information));
    private MainWindow? _window;
    private DiscordClient? _client;
    private ShellViewModel? _shell;
    private SettingsViewModel? _settings;
    private AppNotificationService? _notifications;
    /// <summary>A link to open once signed in (from a notification or swiftcord:// link that launched the app).</summary>
    private DeepLink? _pendingLink;

    public App()
    {
        InitializeComponent();
        Instance = this;
        UnhandledException += (_, e) =>
        {
            // e.Message often carries the XAML error detail that the exception itself lacks.
            _loggerFactory.CreateLogger<App>().LogError(e.Exception, "Unhandled exception: {Message}", e.Message);
            // Keep running: a rendering bug in one message shouldn't close the app.
            e.Handled = true;
        };
    }

    public static new App Current => (App)Application.Current;

    /// <summary>Set once constructed; safe to read from any thread (unlike Application.Current).</summary>
    internal static App? Instance { get; private set; }

    public static string DataDirectory { get; } =
        Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "Swiftcord");

    public ILogger<T> CreateLogger<T>() => _loggerFactory.CreateLogger<T>();

    public MainWindow Window => _window ?? throw new InvalidOperationException("The window hasn't been created yet.");

    protected override void OnLaunched(Microsoft.UI.Xaml.LaunchActivatedEventArgs args)
    {
        _settings = new SettingsViewModel(new JsonSettingsStore(Path.Combine(DataDirectory, "settings.json")), new VelopackUpdateService());
        _notifications = new AppNotificationService(_loggerFactory.CreateLogger<AppNotificationService>());
        _notifications.Invoked += link => Window.DispatcherQueue.TryEnqueue(() => OpenLink(link));
        RegisterProtocol();
        _pendingLink = LinkFrom(AppInstance.GetCurrent().GetActivatedEventArgs());

        _window = new MainWindow();
        _window.ApplyTheme(_settings.Current.Theme);
        _settings.Changed += s => _window.ApplyTheme(s.Theme);
        _window.Activated += (_, e) =>
            _shell?.SetWindowActive(e.WindowActivationState != WindowActivationState.Deactivated);
        _window.Closed += async (_, _) =>
        {
            _notifications?.Dispose();
            await EndSessionAsync(logout: false);
        };
        _window.Activate();

        if (_tokens.Load() is { Length: > 0 } saved)
        {
            var isBot = saved.StartsWith(BotTokenPrefix, StringComparison.Ordinal);
            StartSession(isBot ? saved[BotTokenPrefix.Length..] : saved, isBot ? TokenType.Bot : TokenType.User);
        }
        else
        {
            ShowLogin();
        }

        _ = _settings.CheckOnStartupAsync();
    }

    /// <summary>Handles an activation redirected from a second launch (a link, a notification click, or just reopening).</summary>
    public void HandleActivation(AppActivationArguments args)
    {
        var link = LinkFrom(args);
        _window?.DispatcherQueue.TryEnqueue(() =>
        {
            _window.BringToFront();
            if (link is not null) OpenLink(link);
        });
    }

    private void OpenLink(DeepLink link)
    {
        _window?.BringToFront();
        if (_shell is { } shell)
            shell.Navigate(link);
        else
            _pendingLink = link;
    }

    private static DeepLink? LinkFrom(AppActivationArguments args) => args.Data switch
    {
        IProtocolActivatedEventArgs protocol when DeepLink.TryParse(protocol.Uri, out var link) => link,
        AppNotificationActivatedEventArgs notification => AppNotificationService.LinkFrom(notification),
        // A plain launch with a link on the command line, e.g. `Swiftcord.exe swiftcord://channels/...`.
        ILaunchActivatedEventArgs launch => launch.Arguments.Split(' ', StringSplitOptions.RemoveEmptyEntries)
            .Select(a => DeepLink.TryParse(a.Trim('"'), out var link) ? link : null)
            .FirstOrDefault(l => l is not null),
        _ => null,
    };

    /// <summary>Points swiftcord:// at this executable (per user, no admin needed). Re-run each launch so it follows moves and updates.</summary>
    private void RegisterProtocol()
    {
        try
        {
            if (Environment.ProcessPath is { } exe)
                ActivationRegistrationManager.RegisterForProtocolActivation(DeepLink.Scheme, $"{exe},0", "Swiftcord", exe);
        }
        catch (Exception ex)
        {
            _loggerFactory.CreateLogger<App>().LogWarning(ex, "Couldn't register the swiftcord:// protocol");
        }
    }

    private void ShowLogin()
    {
        var login = new LoginViewModel();
        login.LoggedIn += (token, type, _) =>
        {
            _tokens.Save(type == TokenType.Bot ? BotTokenPrefix + token : token);
            StartSession(token, type);
        };
        Window.ShowLogin(login);
    }

    private async void StartSession(string token, TokenType type)
    {
        var options = new DiscordClientOptions { Token = token, TokenType = type };
        _client = new DiscordClient(options, _loggerFactory);
        _shell = new ShellViewModel(_client, new DispatcherQueueUiDispatcher(Window.DispatcherQueue))
        {
            Notifications = _notifications,
            Settings = _settings,
        };
        if (_pendingLink is { } link)
        {
            _pendingLink = null;
            _shell.Navigate(link);
        }
        _shell.SessionExpired += async () =>
        {
            await EndSessionAsync(logout: false);
            _tokens.Clear();
            ShowLogin();
        };
        _shell.LogoutRequested += async () =>
        {
            await EndSessionAsync(logout: true);
            _tokens.Clear();
            ShowLogin();
        };
        Window.ShowShell(_shell);

        try
        {
            await _client.Gateway.StartAsync();
        }
        catch (Exception ex)
        {
            _loggerFactory.CreateLogger<App>().LogError(ex, "Failed to start the gateway");
        }
    }

    private async Task EndSessionAsync(bool logout)
    {
        var client = _client;
        _shell?.Dispose();
        _shell = null;
        _client = null;
        if (client is null) return;

        if (logout && client.Options.TokenType == TokenType.User)
        {
            try { await client.Rest.LogoutAsync(); }
            catch (Exception ex) { _loggerFactory.CreateLogger<App>().LogWarning(ex, "Logout request failed"); }
        }
        await client.DisposeAsync();
    }
}
