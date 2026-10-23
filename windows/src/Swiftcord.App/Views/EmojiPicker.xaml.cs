using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Controls.Primitives;
using Microsoft.UI.Xaml.Data;
using Microsoft.UI.Xaml.Input;
using Swiftcord.ViewModels.Emojis;
using Windows.System;

namespace Swiftcord.App.Views;

/// <summary>The emoji picker, shown in a flyout for reactions and the composer.</summary>
public sealed partial class EmojiPicker : UserControl
{
    private Flyout? _flyout;

    public EmojiPicker(EmojiPickerViewModel viewModel)
    {
        ViewModel = viewModel;
        InitializeComponent();
        // Each section is a list of its emoji, so a grouped view gives headers with virtualized items.
        SectionsGrid.ItemsSource = new CollectionViewSource { IsSourceGrouped = true, Source = viewModel.Sections }.View;
    }

    public EmojiPickerViewModel ViewModel { get; }

    /// <summary>Opens a picker at <paramref name="anchor"/>; <paramref name="picked"/> runs once with the choice.</summary>
    public static void ShowAt(FrameworkElement anchor, EmojiPickerViewModel viewModel, Action<EmojiItem> picked)
    {
        var picker = new EmojiPicker(viewModel);
        picker._flyout = new Flyout { Content = picker, Placement = FlyoutPlacementMode.TopEdgeAlignedRight };
        picker._flyout.Opened += (_, _) => picker.SearchBox.Focus(FocusState.Programmatic);
        viewModel.Picked += item =>
        {
            picker._flyout.Hide();
            picked(item);
        };
        picker._flyout.ShowAt(anchor);
    }

    private void OnEmojiClick(object sender, ItemClickEventArgs e)
    {
        if (e.ClickedItem is EmojiItem item)
            ViewModel.Pick(item);
    }

    private void OnSearchKeyDown(object sender, KeyRoutedEventArgs e)
    {
        if (e.Key == VirtualKey.Enter && ViewModel.FirstResult is { } first)
        {
            e.Handled = true;
            ViewModel.Pick(first);
        }
    }
}
