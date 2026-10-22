using Microsoft.UI.Xaml;
using Swiftcord.App.Views;
using Swiftcord.ViewModels.Login;
using Swiftcord.ViewModels.Navigation;
using Windows.Graphics;

namespace Swiftcord.App;

public sealed partial class MainWindow : Window
{
    public MainWindow()
    {
        InitializeComponent();
        ExtendsContentIntoTitleBar = true;
        SetTitleBar(AppTitleBar);
        AppWindow.Resize(new SizeInt32(1280, 820));
        AppWindow.TitleBar.PreferredHeightOption = Microsoft.UI.Windowing.TitleBarHeightOption.Tall;
    }

    /// <summary>The native window handle, for WinRT pickers that need an owner.</summary>
    public nint Handle => WinRT.Interop.WindowNative.GetWindowHandle(this);

    public void ShowLogin(LoginViewModel viewModel) => RootContent.Content = new LoginPage(viewModel);

    public void ShowShell(ShellViewModel viewModel) => RootContent.Content = new ShellPage(viewModel);
}
