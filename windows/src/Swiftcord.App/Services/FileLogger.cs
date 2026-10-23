using System.Collections.Concurrent;
using Microsoft.Extensions.Logging;

namespace Swiftcord.App.Services;

/// <summary>
/// Writes log lines to %LOCALAPPDATA%\Swiftcord\Logs\swiftcord.log so problems can be diagnosed without a debugger.
/// The previous session's log is kept as swiftcord.old.log.
/// </summary>
public sealed class FileLoggerProvider : ILoggerProvider
{
    private readonly StreamWriter? _writer;
    private readonly Lock _lock = new();
    private readonly ConcurrentDictionary<string, FileLogger> _loggers = new();

    public FileLoggerProvider()
    {
        try
        {
            Directory.CreateDirectory(LogDirectory);
            var path = Path.Combine(LogDirectory, "swiftcord.log");
            if (File.Exists(path))
                File.Copy(path, Path.Combine(LogDirectory, "swiftcord.old.log"), overwrite: true);
            _writer = new StreamWriter(new FileStream(path, FileMode.Create, FileAccess.Write, FileShare.ReadWrite)) { AutoFlush = true };
        }
        catch (IOException)
        {
            // Another instance holds the file; logging to disk is best-effort.
        }
        catch (UnauthorizedAccessException)
        {
        }
    }

    public static string LogDirectory { get; } =
        Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "Swiftcord", "Logs");

    public ILogger CreateLogger(string categoryName) => _loggers.GetOrAdd(categoryName, name => new FileLogger(this, name));

    public void Dispose()
    {
        lock (_lock) _writer?.Dispose();
    }

    private void Write(string line)
    {
        lock (_lock) _writer?.WriteLine(line);
    }

    private sealed class FileLogger(FileLoggerProvider provider, string category) : ILogger
    {
        // Keep "Swiftcord.App.Views.ChatView" readable as "ChatView".
        private readonly string _name = category[(category.LastIndexOf('.') + 1)..];

        public IDisposable? BeginScope<TState>(TState state) where TState : notnull => null;

        public bool IsEnabled(LogLevel logLevel) => provider._writer is not null && logLevel != LogLevel.None;

        public void Log<TState>(LogLevel logLevel, EventId eventId, TState state, Exception? exception, Func<TState, Exception?, string> formatter)
        {
            if (!IsEnabled(logLevel)) return;
            var line = $"{DateTime.Now:HH:mm:ss.fff} {logLevel,-11} {_name}: {formatter(state, exception)}";
            if (exception is not null)
                line += Environment.NewLine + exception;
            provider.Write(line);
        }
    }
}
