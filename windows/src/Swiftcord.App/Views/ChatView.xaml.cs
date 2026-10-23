using System.ComponentModel;
using Microsoft.Extensions.Logging;
using Microsoft.UI.Input;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Input;
using Microsoft.UI.Xaml.Media;
using Swiftcord.ViewModels.Messages;
using Windows.ApplicationModel.DataTransfer;
using Windows.Foundation;
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

    private readonly ILogger<ChatView> _log = App.Current.CreateLogger<ChatView>();
    private ScrollViewer? _scroller;
    private bool _loadingOlder;
    private bool _loadingNewer;
    private int _scrollToVersion;

    public ChatView()
    {
        InitializeComponent();
        Unloaded += (_, _) =>
        {
            ViewModel = null; // Unsubscribes from the view model, which outlives this view on a theme change.
            Bindings.StopTracking();
        };
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
        if (e.PropertyName == nameof(ChatViewModel.IsLoading) && ViewModel is { IsLoading: false } loaded)
        {
            // After a jump, show the target rather than the newest message.
            if (loaded.Highlighted is { IsHighlighted: true } target)
                ScrollToMessage(target);
            else
                ScrollToBottom();
        }
        else if (e.PropertyName == nameof(ChatViewModel.Highlighted) && ViewModel?.Highlighted is { } highlighted)
            ScrollToMessage(highlighted);
        else if (e.PropertyName == nameof(ChatViewModel.Draft) && ViewModel is { Draft.Length: 0 })
            ScrollToBottom(); // A message was just sent.
    }

    /// <summary>
    /// Brings a message into view a third of the way down, so the context above it shows too. Rows that
    /// haven't been drawn yet are estimated, so the first scroll can land off target; measure where the row
    /// actually is and correct, a few times while the rows around it are realised.
    /// </summary>
    private async void ScrollToMessage(MessageItemViewModel item)
    {
        var version = ++_scrollToVersion;
        for (var attempt = 0; attempt < 4; attempt++)
        {
            if (version != _scrollToVersion || ViewModel?.Messages.Contains(item) != true || _scroller is not { } scroller)
                return;
            var container = MessageList.ContainerFromItem(item) as UIElement;
            if (container is null)
            {
                // Not realised yet: get it on screen first, then measure.
                MessageList.ScrollIntoView(item, ScrollIntoViewAlignment.Leading);
                MessageList.UpdateLayout();
                container = MessageList.ContainerFromItem(item) as UIElement;
            }
            if (container is not null)
            {
                var top = container.TransformToVisual(scroller).TransformPoint(new Point(0, 0)).Y;
                var target = Math.Max(0, scroller.VerticalOffset + top - scroller.ViewportHeight / 3);
                if (Math.Abs(target - scroller.VerticalOffset) > 2)
                    scroller.ChangeView(null, target, null, disableAnimation: true);
            }
            await Task.Delay(60);
        }
    }

    private void ScrollToBottom()
    {
        _scrollToVersion++; // Cancel any pending jump correction.
        if (ViewModel?.Messages.LastOrDefault() is { } last)
            MessageList.ScrollIntoView(last, ScrollIntoViewAlignment.Leading);
    }

    #region History paging

    private void OnMessageListLoaded(object sender, RoutedEventArgs e)
    {
        if (!TryAttachScroller())
            MessageList.LayoutUpdated += OnMessageListLayoutUpdated;
    }

    /// <summary>
    /// The view starts collapsed until a channel opens, and a collapsed ListView hasn't built its template yet,
    /// so the ScrollViewer may only appear after a later layout pass.
    /// </summary>
    private void OnMessageListLayoutUpdated(object? sender, object e)
    {
        if (TryAttachScroller())
            MessageList.LayoutUpdated -= OnMessageListLayoutUpdated;
    }

    private bool TryAttachScroller()
    {
        if (_scroller is not null) return true;
        _scroller = FindDescendant<ScrollViewer>(MessageList);
        if (_scroller is null) return false;

        _log.LogInformation("Attached to the message list's ScrollViewer");
        _scroller.ViewChanged += OnScrollViewChanged;
        // Wheeling while already at the top raises no ViewChanged, so listen for the wheel itself too.
        _scroller.AddHandler(PointerWheelChangedEvent, new PointerEventHandler(OnScrollerWheel), handledEventsToo: true);
        return true;
    }

    private void OnScrollViewChanged(object? sender, ScrollViewerViewChangedEventArgs e)
    {
        _ = LoadOlderWhileNearTopAsync();
        _ = LoadNewerIfNearBottomAsync();
    }

    /// <summary>While browsing older history (after a jump), scrolling to the bottom pages towards the present.</summary>
    private async Task LoadNewerIfNearBottomAsync()
    {
        if (_loadingNewer || _scroller is not { } scroller
            || scroller.VerticalOffset < scroller.ExtentHeight - scroller.ViewportHeight - LoadOlderThreshold
            || ViewModel is not { IsViewingHistory: true, IsLoading: false } vm || !vm.LoadNewerCommand.CanExecute(null))
            return;

        _loadingNewer = true;
        try
        {
            // Messages are appended below. The list keeps the last item in view when it was visible, which would
            // jump to the new bottom and trigger the next page in a cascade, so put the view back where it was.
            var offset = scroller.VerticalOffset;
            await vm.LoadNewerCommand.ExecuteAsync(null);
            if (!ReferenceEquals(ViewModel, vm)) return;
            MessageList.UpdateLayout();
            if (vm.IsViewingHistory)
                scroller.ChangeView(null, offset, null, disableAnimation: true);
        }
        catch (Exception ex)
        {
            _log.LogError(ex, "Loading newer history failed");
        }
        finally
        {
            _loadingNewer = false;
        }
    }

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
            if (offset < LoadOlderThreshold && ViewModel is { } current)
                _log.LogInformation("Near top: offset {Offset:F0}, extent {Extent:F0}, {Count} messages, loading {Loading}, more {More}, can load {CanLoad}, error {Error}",
                    offset, scroller.ExtentHeight, current.Messages.Count, current.IsLoading, current.HasMoreHistory,
                    current.LoadOlderCommand.CanExecute(null), current.Error ?? "none");
            while (offset < LoadOlderThreshold
                && ViewModel is { IsLoading: false, HasMoreHistory: true } vm && vm.LoadOlderCommand.CanExecute(null))
            {
                // Older messages are inserted above the viewport. Restore the same distance from the bottom
                // afterwards so the visible messages don't move.
                var fromBottom = scroller.ExtentHeight - offset;
                var before = vm.Messages.Count;
                await vm.LoadOlderCommand.ExecuteAsync(null);
                _log.LogInformation("Loaded older page: {Before} -> {After} messages, more {More}, error {Error}",
                    before, vm.Messages.Count, vm.HasMoreHistory, vm.Error ?? "none");
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
                _log.LogInformation("After layout: offset {Offset:F0}, extent {Extent:F0}, viewport {Viewport:F0}",
                    offset, scroller.ExtentHeight, scroller.ViewportHeight);
            }
        }
        catch (Exception ex)
        {
            _log.LogError(ex, "Loading older history failed");
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
