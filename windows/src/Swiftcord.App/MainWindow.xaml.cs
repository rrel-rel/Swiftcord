using System.Runtime.InteropServices;
using Microsoft.UI;
using Microsoft.UI.Windowing;
using Microsoft.UI.Xaml;
using Swiftcord.App.Helpers;
using Swiftcord.App.Views;
using Swiftcord.ViewModels.Login;
using Swiftcord.ViewModels.Navigation;
using Swiftcord.ViewModels.Settings;
using Windows.Graphics;
using Windows.UI;

namespace Swiftcord.App;

public sealed partial class MainWindow : Window
{
    /// <summary>Recreates the current page; used to redraw it in a new theme.</summary>
    private Func<UIElement>? _createContent;

    public MainWindow()
    {
        InitializeComponent();
        ExtendsContentIntoTitleBar = true;
        SetTitleBar(AppTitleBar);
        AppWindow.Resize(new SizeInt32(1280, 820));
        AppWindow.TitleBar.PreferredHeightOption = TitleBarHeightOption.Tall;
        AppWindow.TitleBar.ButtonBackgroundColor = Colors.Transparent;
        AppWindow.TitleBar.ButtonInactiveBackgroundColor = Colors.Transparent;
        Ui.Theme = Root.ActualTheme;
        Root.ActualThemeChanged += (_, _) => OnThemeChanged();
        UpdateCaptionButtons();
    }

    /// <summary>Applies the app theme to the window content (the system theme applies to "System").</summary>
    public void ApplyTheme(AppTheme theme)
    {
        Root.RequestedTheme = theme switch
        {
            AppTheme.Light => ElementTheme.Light,
            AppTheme.Dark => ElementTheme.Dark,
            _ => ElementTheme.Default,
        };
    }

    /// <summary>
    /// Colours set from code and x:Bind functions don't follow theme changes the way {ThemeResource} does,
    /// so the page is rebuilt from its view model (which keeps all state) in the new theme.
    /// </summary>
    private void OnThemeChanged()
    {
        UpdateCaptionButtons();
        if (Ui.Theme == Root.ActualTheme) return;
        Ui.Theme = Root.ActualTheme;
        if (_createContent is { } create)
            RootContent.Content = create();
    }

    /// <summary>Caption buttons are drawn by the system and don't follow the element theme on their own.</summary>
    private void UpdateCaptionButtons()
    {
        var dark = Root.ActualTheme == ElementTheme.Dark;
        var bar = AppWindow.TitleBar;
        bar.ButtonForegroundColor = dark ? Colors.White : Colors.Black;
        bar.ButtonHoverForegroundColor = dark ? Colors.White : Colors.Black;
        bar.ButtonHoverBackgroundColor = dark ? Color.FromArgb(0x15, 0xFF, 0xFF, 0xFF) : Color.FromArgb(0x09, 0, 0, 0);
        bar.ButtonPressedBackgroundColor = dark ? Color.FromArgb(0x0F, 0xFF, 0xFF, 0xFF) : Color.FromArgb(0x06, 0, 0, 0);
        bar.ButtonInactiveForegroundColor = dark ? Color.FromArgb(0x87, 0xFF, 0xFF, 0xFF) : Color.FromArgb(0x72, 0, 0, 0);
    }

    /// <summary>Restores and focuses the window, e.g. when a link or notification is opened from elsewhere.</summary>
    public void BringToFront()
    {
        if (AppWindow.Presenter is OverlappedPresenter { State: OverlappedPresenterState.Minimized } presenter)
            presenter.Restore();
        Activate();
        SetForegroundWindow(Handle);
    }

    [DllImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool SetForegroundWindow(nint hwnd);

    /// <summary>The native window handle, for WinRT pickers that need an owner.</summary>
    public nint Handle => WinRT.Interop.WindowNative.GetWindowHandle(this);

    public void ShowLogin(LoginViewModel viewModel) => Show(() => new LoginPage(viewModel));

    public void ShowShell(ShellViewModel viewModel) => Show(() => new ShellPage(viewModel));

    private void Show(Func<UIElement> create)
    {
        _createContent = create;
        RootContent.Content = create();
    }
}
