namespace Swiftcord.Core.Auth;

/// <summary>
/// Persists the account token between launches. The Windows app implements this with the
/// Credential Locker so tokens never sit in plain-text settings.
/// </summary>
public interface ITokenStore
{
    string? Load();

    void Save(string token);

    void Clear();
}

/// <summary>Non-persistent store for tests and first-run flows.</summary>
public sealed class InMemoryTokenStore : ITokenStore
{
    private string? _token;

    public string? Load() => _token;

    public void Save(string token) => _token = token;

    public void Clear() => _token = null;
}
