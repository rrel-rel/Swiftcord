using System.ComponentModel;
using Microsoft.UI.Input;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Input;
using Microsoft.UI.Xaml.Media;
using Swiftcord.ViewModels.Messages;
using Windows.ApplicationModel.DataTransfer;
using Windows.Storage;
using Windows.Storage.Pickers;
using Windows.System;
using Windows.UI.Core;

namespace Swiftcord.App.Views;

public sealed partial class ChatView : UserControl
{
    /// <summary>Start fetching older history when scrolled within this distance of the top.</summary>
    private const double LoadOlderThreshold = 600;

    public static readonly DependencyProperty ViewModelProperty = DependencyProperty.Register(
        nameof(ViewModel), typeof(ChatViewModel), typeof(ChatView), new PropertyMetadata(null, OnViewModelChanged));

    private ScrollViewer? _scroller;
    private bool _loadingOlder;

    public ChatView()
    {
        InitializeComponent();
    }

    public ChatViewModel? ViewModel
    {
        get => (ChatViewModel?)GetValue(ViewModelProperty);
        set => SetValue(ViewModelProperty, value);
    }

    private static void OnViewModelChanged(DependencyObject d, DependencyPropertyChangedEventArgs e)
    {
        var view = (ChatView)d;
        if (e.OldValue is ChatViewModel old)
            old.PropertyChanged -= view.OnViewModelPropertyChanged;
        if (e.NewValue is ChatViewModel current)
        {
            current.PropertyChanged += view.OnViewModelPropertyChanged;
            view.ComposerBox.Focus(FocusState.Programmatic);
        }
    }

    private void OnViewModelPropertyChanged(object? sender, PropertyChangedEventArgs e)
    {
        if (e.PropertyName == nameof(ChatViewModel.IsLoading) && ViewModel is { IsLoading: false })
            ScrollToBottom();
        else if (e.PropertyName == nameof(ChatViewModel.Draft) && ViewModel is { Draft.Length: 0 })
            ScrollToBottom(); // A message was just sent.
    }

    private void ScrollToBottom()
    {
        if (ViewModel?.Messages.LastOrDefault() is { } last)
            MessageList.ScrollIntoView(last, ScrollIntoViewAlignment.Leading);
    }

    #region History paging

    private void OnMessageListLoaded(object sender, RoutedEventArgs e)
    {
        if (_scroller is not null) return;
        _scroller = FindDescendant<ScrollViewer>(MessageList);
        if (_scroller is null) return;
        _scroller.ViewChanged += OnScrollViewChanged;
        // Wheeling while already at the top raises no ViewChanged, so listen for the wheel itself too.
        _scroller.AddHandler(PointerWheelChangedEvent, new PointerEventHandler(OnScrollerWheel), handledEventsToo: true);
    }

    private void OnScrollViewChanged(object? sender, ScrollViewerViewChangedEventArgs e) => _ = LoadOlderWhileNearTopAsync();

    private void OnScrollerWheel(object sender, PointerRoutedEventArgs e) => _ = LoadOlderWhileNearTopAsync();

    /// <summary>
    /// Pages in older history until the viewport is no longer near the top. Looping here (rather than waiting
    /// for the next scroll event) matters: if the user reaches the top while a page is loading, the event that
    /// would have triggered the next page was skipped and none follows, leaving a blank header and no progress.
    /// </summary>
    private async Task LoadOlderWhileNearTopAsync()
    {
        if (_loadingOlder || _scroller is not { } scroller)
            return;

        _loadingOlder = true;
        try
        {
            var offset = scroller.VerticalOffset;
            while (offset < LoadOlderThreshold
                && ViewModel is { IsLoading: false, HasMoreHistory: true } vm && vm.LoadOlderCommand.CanExecute(null))
            {
                // Older messages are inserted above the viewport. Restore the same distance from the bottom
                // afterwards so the visible messages don't move.
                var fromBottom = scroller.ExtentHeight - offset;
                var before = vm.Messages.Count;
                await vm.LoadOlderCommand.ExecuteAsync(null);
                if (!ReferenceEquals(ViewModel, vm) || vm.Error is not null || vm.Messages.Count == before)
                    break;
                MessageList.UpdateLayout();
                offset = scroller.VerticalOffset;
                var target = scroller.ExtentHeight - fromBottom;
                if (target > offset + 1)
                {
                    scroller.ChangeView(null, target, null, disableAnimation: true);
                    offset = target; // ChangeView applies asynchronously; judge the next iteration by where we're going.
                }
            }
        }
        finally
        {
            _loadingOlder = false;
        }
    }

    private static T? FindDescendant<T>(DependencyObject root) where T : DependencyObject
    {
        for (var i = 0; i < VisualTreeHelper.GetChildrenCount(root); i++)
        {
            var child = VisualTreeHelper.GetChild(root, i);
            if (child is T match) return match;
            if (FindDescendant<T>(child) is { } nested) return nested;
        }
        return null;
    }

    #endregion

    #region Composer

    private void OnComposerKeyDown(object sender, KeyRoutedEventArgs e)
    {
        if (ViewModel is not { } vm) return;
        var shift = InputKeyboardSource.GetKeyStateForCurrentThread(VirtualKey.Shift).HasFlag(CoreVirtualKeyStates.Down);

        switch (e.Key)
        {
            // Enter sends; Shift+Enter inserts a newline.
            case VirtualKey.Enter when !shift:
                e.Handled = true;
                if (vm.SendCommand.CanExecute(null))
                    vm.SendCommand.Execute(null);
                break;
            case VirtualKey.Escape when vm.IsReplying || vm.IsEditing:
                e.Handled = true;
                vm.CancelReplyOrEditCommand.Execute(null);
                break;
            case VirtualKey.Up when ComposerBox.Text.Length == 0:
                e.Handled = vm.EditLastOwnMessage();
                if (e.Handled)
                    ComposerBox.SelectionStart = ComposerBox.Text.Length;
                break;
        }
    }

    private async void OnAttachClick(object sender, RoutedEventArgs e)
    {
        var picker = new FileOpenPicker { ViewMode = PickerViewMode.Thumbnail };
        picker.FileTypeFilter.Add("*");
        WinRT.Interop.InitializeWithWindow.Initialize(picker, App.Current.Window.Handle);
        foreach (var file in await picker.PickMultipleFilesAsync())
            AddFile(file);
    }

    private void OnDragOver(object sender, DragEventArgs e)
    {
        if (ViewModel is { CanAttach: true } && e.DataView.Contains(StandardDataFormats.StorageItems))
        {
            e.AcceptedOperation = DataPackageOperation.Copy;
            e.DragUIOverride.Caption = "Upload to " + ViewModel.Title;
        }
    }

    private async void OnDrop(object sender, DragEventArgs e)
    {
        if (!e.DataView.Contains(StandardDataFormats.StorageItems)) return;
        foreach (var item in await e.DataView.GetStorageItemsAsync())
        {
            if (item is StorageFile file)
                AddFile(file);
        }
    }

    private void AddFile(StorageFile file)
    {
        var size = new FileInfo(file.Path).Length;
        ViewModel?.AddAttachment(new PendingAttachment(file.Name, size,
            () => Task.FromResult<Stream>(File.OpenRead(file.Path)), file.ContentType));
    }

    #endregion

    #region Message actions

    private async void OnOpenUri(object sender, RoutedEventArgs e)
    {
        if (sender is FrameworkElement { Tag: Uri uri })
            await Launcher.LaunchUriAsync(uri);
    }

    private void OnRevealSpoiler(object sender, TappedRoutedEventArgs e)
    {
        ((UIElement)sender).Visibility = Visibility.Collapsed;
        e.Handled = true;
    }

    private void OnCopyText(object sender, RoutedEventArgs e)
    {
        if (sender is FrameworkElement { Tag: string text })
        {
            var package = new DataPackage();
            package.SetText(text);
            Clipboard.SetContent(package);
        }
    }

    private void OnErrorClosed(InfoBar sender, object args)
    {
        if (ViewModel is { } vm)
            vm.Error = null;
    }

    #endregion
}
