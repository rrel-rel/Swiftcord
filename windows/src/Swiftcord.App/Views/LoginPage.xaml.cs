using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.Web.WebView2.Core;
using Swiftcord.ViewModels.Login;

namespace Swiftcord.App.Views;

public sealed partial class LoginPage : UserControl
{
    private const string LoginUrl = "https://discord.com/login";

    public LoginPage(LoginViewModel viewModel)
    {
        ViewModel = viewModel;
        InitializeComponent();
        Loaded += OnLoaded;
        Unloaded += (_, _) => LoginWebView.Close();
    }

    public LoginViewModel ViewModel { get; }

    private async void OnLoaded(object sender, RoutedEventArgs e)
    {
        try
        {
            // Unpackaged apps may live in a read-only folder, so keep browser data in LocalAppData.
            var dataFolder = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "Swiftcord", "WebView2");
            var environment = await CoreWebView2Environment.CreateWithOptionsAsync(null, dataFolder, new CoreWebView2EnvironmentOptions());
            await LoginWebView.EnsureCoreWebView2Async(environment);
            var core = LoginWebView.CoreWebView2;

            // Start signed out, otherwise a previous web session would log straight back in after "Log out".
            await core.Profile.ClearBrowsingDataAsync();

            core.Settings.AreDevToolsEnabled = false;
            core.Settings.IsStatusBarEnabled = false;
            // Once signed in, the page calls the API with the token in the Authorization header.
            core.AddWebResourceRequestedFilter("https://discord.com/api/*", CoreWebView2WebResourceContext.All);
            core.WebResourceRequested += OnWebResourceRequested;
            core.NavigationCompleted += (_, _) => WebViewLoading.IsActive = false;
            core.NewWindowRequested += (_, args) =>
            {
                // Keep "Forgot password" and similar links in the same view.
                args.Handled = true;
                core.Navigate(args.Uri);
            };
            core.Navigate(LoginUrl);
        }
        catch (Exception)
        {
            WebViewLoading.IsActive = false;
            ViewModel.Error = "The Microsoft Edge WebView2 Runtime is needed to sign in. Install it, or sign in with a token below.";
        }
    }

    private async void OnWebResourceRequested(CoreWebView2 sender, CoreWebView2WebResourceRequestedEventArgs args)
    {
        var headers = args.Request.Headers;
        if (!headers.Contains("Authorization"))
            return;
        var authorization = headers.GetHeader("Authorization");
        if (await ViewModel.OnTokenObservedAsync(authorization))
            sender.WebResourceRequested -= OnWebResourceRequested;
    }
}
