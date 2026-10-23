using Microsoft.UI.Dispatching;
using Microsoft.UI.Xaml;
using Microsoft.Windows.AppLifecycle;
using Velopack;

namespace Swiftcord.App;

/// <summary>
/// Custom entry point (DISABLE_XAML_GENERATED_MAIN): runs Velopack's install/update hooks, and keeps a
/// single instance, so a swiftcord:// link or notification click goes to the running window.
/// </summary>
public static class Program
{
    private const string InstanceKey = "Swiftcord.Main";

    [STAThread]
    private static int Main(string[] args)
    {
        // Must run first: during install/update/uninstall Velopack starts the app with hook arguments and exits here.
        VelopackApp.Build().Run();

        WinRT.ComWrappersSupport.InitializeComWrappers();
        if (RedirectToRunningInstance())
            return 0;

        Application.Start(callback =>
        {
            var context = new DispatcherQueueSynchronizationContext(DispatcherQueue.GetForCurrentThread());
            SynchronizationContext.SetSynchronizationContext(context);
            _ = new App();
        });
        return 0;
    }

    /// <summary>True when another instance is running and has been handed this activation.</summary>
    private static bool RedirectToRunningInstance()
    {
        var activation = AppInstance.GetCurrent().GetActivatedEventArgs();
        var main = AppInstance.FindOrRegisterForKey(InstanceKey);
        if (main.IsCurrent)
        {
            main.Activated += (_, redirected) => App.Instance?.HandleActivation(redirected);
            return false;
        }

        // Block until the redirect is delivered; the STA wait pumps COM messages, so this can't deadlock.
        Task.Run(() => main.RedirectActivationToAsync(activation).AsTask()).Wait();
        return true;
    }
}
