using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Controls.Primitives;
using Swiftcord.ViewModels.Profiles;

namespace Swiftcord.App.Views;

/// <summary>A user's profile, shown in a flyout next to whatever was clicked (avatar, name or @mention).</summary>
public sealed partial class ProfileCard : UserControl
{
    private Flyout? _flyout;

    public ProfileCard(ProfileViewModel viewModel)
    {
        ViewModel = viewModel;
        InitializeComponent();
    }

    public ProfileViewModel ViewModel { get; }

    public static void ShowAt(FrameworkElement anchor, ProfileViewModel viewModel)
    {
        var card = new ProfileCard(viewModel);
        var presenterStyle = new Style(typeof(FlyoutPresenter));
        presenterStyle.Setters.Add(new Setter(PaddingProperty, new Thickness(0)));
        presenterStyle.Setters.Add(new Setter(MaxWidthProperty, 360.0));
        presenterStyle.Setters.Add(new Setter(CornerRadiusProperty, new CornerRadius(8)));
        card._flyout = new Flyout
        {
            Content = card,
            Placement = FlyoutPlacementMode.RightEdgeAlignedTop,
            FlyoutPresenterStyle = presenterStyle,
        };
        card._flyout.ShowAt(anchor);
        _ = viewModel.LoadAsync();
    }

    private async void OnMessageClick(object sender, RoutedEventArgs e)
    {
        await ViewModel.SendMessageCommand.ExecuteAsync(null);
        if (ViewModel.Error is null)
            _flyout?.Hide();
    }
}
