using Microsoft.Extensions.Logging;
using Microsoft.UI.Xaml;
using Swiftcord.App.Services;
using Swiftcord.Core;
using Swiftcord.Core.Auth;
using Swiftcord.ViewModels.Login;
using Swiftcord.ViewModels.Navigation;

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

    public App()
    {
        InitializeComponent();
        UnhandledException += (_, e) =>
        {
            // e.Message often carries the XAML error detail that the exception itself lacks.
            _loggerFactory.CreateLogger<App>().LogError(e.Exception, "Unhandled exception: {Message}", e.Message);
            // Keep running: a rendering bug in one message shouldn't close the app.
            e.Handled = true;
        };
    }

    public static new App Current => (App)Application.Current;

    public ILogger<T> CreateLogger<T>() => _loggerFactory.CreateLogger<T>();

    public MainWindow Window => _window ?? throw new InvalidOperationException("The window hasn't been created yet.");

    protected override void OnLaunched(LaunchActivatedEventArgs args)
    {
        _window = new MainWindow();
        _window.Activated += (_, e) =>
            _shell?.SetWindowActive(e.WindowActivationState != WindowActivationState.Deactivated);
        _window.Closed += async (_, _) => await EndSessionAsync(logout: false);
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
        _shell = new ShellViewModel(_client, new DispatcherQueueUiDispatcher(Window.DispatcherQueue));
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
