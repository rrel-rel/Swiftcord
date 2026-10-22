using Microsoft.UI.Dispatching;
using Swiftcord.Core.Auth;
using Swiftcord.ViewModels;
using Windows.Security.Credentials;

namespace Swiftcord.App.Services;

/// <summary>Posts work to the window's DispatcherQueue (running inline when already on the UI thread).</summary>
public sealed class DispatcherQueueUiDispatcher(DispatcherQueue queue) : IUiDispatcher
{
    public void Post(Action action)
    {
        if (queue.HasThreadAccess)
            action();
        else
            queue.TryEnqueue(() => action());
    }
}

/// <summary>Keeps the token in the Windows Credential Locker (visible in Credential Manager).</summary>
public sealed class CredentialTokenStore : ITokenStore
{
    private const string Resource = "Swiftcord";
    private const string UserName = "token";

    private readonly PasswordVault _vault = new();

    public string? Load()
    {
        try
        {
            var credential = _vault.Retrieve(Resource, UserName);
            credential.RetrievePassword();
            return credential.Password;
        }
        catch (Exception)
        {
            // Retrieve throws when there's no stored credential.
            return null;
        }
    }

    public void Save(string token)
    {
        Clear();
        _vault.Add(new PasswordCredential(Resource, UserName, token));
    }

    public void Clear()
    {
        try
        {
            foreach (var credential in _vault.FindAllByResource(Resource))
                _vault.Remove(credential);
        }
        catch (Exception)
        {
            // FindAllByResource throws when nothing is stored.
        }
    }
}
