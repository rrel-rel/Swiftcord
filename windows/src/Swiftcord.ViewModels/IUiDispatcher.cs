namespace Swiftcord.ViewModels;

/// <summary>
/// Runs work on the UI thread. Gateway events arrive on a background thread, so view models
/// post every collection or property change through this.
/// </summary>
public interface IUiDispatcher
{
    void Post(Action action);
}

/// <summary>Runs actions inline; for tests and single-threaded hosts.</summary>
public sealed class ImmediateUiDispatcher : IUiDispatcher
{
    public static ImmediateUiDispatcher Instance { get; } = new();

    public void Post(Action action) => action();
}
