using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Input;
using Swiftcord.App.Services;
using Swiftcord.ViewModels.Navigation;
using Windows.System;

namespace Swiftcord.App.Views;

/// <summary>The settings overlay: account, appearance, notifications, updates and troubleshooting.</summary>
public sealed partial class SettingsView : UserControl
{
    public static readonly DependencyProperty ShellProperty = DependencyProperty.Register(
        nameof(Shell), typeof(ShellViewModel), typeof(SettingsView), new PropertyMetadata(null, (d, _) => ((SettingsView)d).Bindings.Update()));

    public SettingsView()
    {
        InitializeComponent();
        // Move focus in when shown, so Escape (handled below) reaches this view.
        RegisterPropertyChangedCallback(VisibilityProperty, (_, _) =>
        {
            if (Visibility == Visibility.Visible)
                CloseButton.Focus(FocusState.Programmatic);
        });
    }

    public ShellViewModel? Shell
    {
        get => (ShellViewModel?)GetValue(ShellProperty);
        set => SetValue(ShellProperty, value);
    }

    private void OnKeyDown(object sender, KeyRoutedEventArgs e)
    {
        if (e.Key == VirtualKey.Escape && Shell is { } shell)
        {
            shell.CloseSettingsCommand.Execute(null);
            e.Handled = true;
        }
    }

    private async void OnOpenLogsClick(object sender, RoutedEventArgs e)
    {
        Directory.CreateDirectory(FileLoggerProvider.LogDirectory);
        await Launcher.LaunchFolderPathAsync(FileLoggerProvider.LogDirectory);
    }
}
