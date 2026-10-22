using CommunityToolkit.Mvvm.ComponentModel;
using CommunityToolkit.Mvvm.Input;
using Swiftcord.Core;
using Swiftcord.Core.Models;
using Swiftcord.Core.Rest;

namespace Swiftcord.ViewModels.Login;

/// <summary>
/// Validates a token before it's stored. Tokens arrive either from the embedded discord.com
/// login page (captured from its API requests) or pasted manually.
/// </summary>
public sealed partial class LoginViewModel(Func<DiscordClientOptions, DiscordRestClient>? restFactory = null) : ObservableObject
{
    private readonly Func<DiscordClientOptions, DiscordRestClient> _restFactory = restFactory ?? (o => new DiscordRestClient(o));
    private string? _lastRejected;

    [ObservableProperty]
    [NotifyCanExecuteChangedFor(nameof(SubmitManualTokenCommand))]
    private string _manualToken = "";

    [ObservableProperty]
    private bool _isBotToken;

    [ObservableProperty]
    [NotifyCanExecuteChangedFor(nameof(SubmitManualTokenCommand))]
    private bool _isBusy;

    [ObservableProperty]
    private string? _error;

    /// <summary>Raised once a token has been verified against the API.</summary>
    public event Action<string, TokenType, User>? LoggedIn;

    /// <summary>
    /// Called with the Authorization header of each API request the login page makes.
    /// Returns true once a working token has been found.
    /// </summary>
    public async Task<bool> OnTokenObservedAsync(string? authorization)
    {
        if (string.IsNullOrWhiteSpace(authorization) || IsBusy || authorization == _lastRejected)
            return false;
        return await TryLoginAsync(authorization.Trim(), TokenType.User, reportErrors: false);
    }

    private bool CanSubmit() => !IsBusy && !string.IsNullOrWhiteSpace(ManualToken);

    [RelayCommand(CanExecute = nameof(CanSubmit))]
    private async Task SubmitManualTokenAsync()
    {
        var token = ManualToken.Trim().Trim('"');
        var type = IsBotToken ? TokenType.Bot : TokenType.User;
        if (type == TokenType.Bot && token.StartsWith("Bot ", StringComparison.Ordinal))
            token = token[4..];
        if (await TryLoginAsync(token, type, reportErrors: true))
            ManualToken = "";
    }

    private async Task<bool> TryLoginAsync(string token, TokenType type, bool reportErrors)
    {
        IsBusy = true;
        Error = null;
        try
        {
            using var rest = _restFactory(new DiscordClientOptions { Token = token, TokenType = type });
            var user = await rest.GetCurrentUserAsync();
            LoggedIn?.Invoke(token, type, user);
            return true;
        }
        catch (DiscordApiException ex) when (ex.Status == System.Net.HttpStatusCode.Unauthorized)
        {
            _lastRejected = token;
            if (reportErrors) Error = "That token didn't work. Check it and try again.";
            return false;
        }
        catch (Exception)
        {
            if (reportErrors) Error = "Couldn't reach Discord. Check your connection and try again.";
            return false;
        }
        finally
        {
            IsBusy = false;
        }
    }
}
