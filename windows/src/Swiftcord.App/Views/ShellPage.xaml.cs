using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Input;
using Swiftcord.ViewModels.Navigation;

namespace Swiftcord.App.Views;

public sealed partial class ShellPage : UserControl
{
    public ShellPage(ShellViewModel viewModel)
    {
        ViewModel = viewModel;
        InitializeComponent();
        // The page is replaced (e.g. on a theme change) while the view model lives on; stop listening to it.
        Unloaded += (_, _) => Bindings.StopTracking();
    }

    public ShellViewModel ViewModel { get; }

    private void OnHomeTapped(object sender, TappedRoutedEventArgs e) => ViewModel.SelectHomeCommand.Execute(null);

    private void OnGuildClick(object sender, ItemClickEventArgs e) =>
        ViewModel.SelectGuildCommand.Execute((GuildItemViewModel)e.ClickedItem);

    private void OnChannelClick(object sender, ItemClickEventArgs e) =>
        ViewModel.ActivateChannelCommand.Execute((ChannelItemViewModel)e.ClickedItem);
}

/// <summary>Chooses the sidebar row template for categories, guild channels and DMs.</summary>
public sealed partial class ChannelTemplateSelector : DataTemplateSelector
{
    public DataTemplate? Category { get; set; }
    public DataTemplate? Channel { get; set; }
    public DataTemplate? DirectMessage { get; set; }

    protected override DataTemplate? SelectTemplateCore(object item) => item switch
    {
        ChannelItemViewModel { IsCategory: true } => Category,
        ChannelItemViewModel { IsPrivate: true } => DirectMessage,
        _ => Channel,
    };

    protected override DataTemplate? SelectTemplateCore(object item, DependencyObject container) => SelectTemplateCore(item);
}
