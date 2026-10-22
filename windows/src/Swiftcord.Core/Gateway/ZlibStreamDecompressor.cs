using System.IO.Compression;

namespace Swiftcord.Core.Gateway;

/// <summary>
/// Decompresses Discord's zlib-stream transport compression, where a single zlib context spans the
/// whole connection and each complete message ends with a Z_SYNC_FLUSH marker (00 00 FF FF).
/// </summary>
internal sealed class ZlibStreamDecompressor : IDisposable
{
    private static ReadOnlySpan<byte> SyncFlushSuffix => [0x00, 0x00, 0xFF, 0xFF];

    private readonly MemoryStream _compressed = new();
    private readonly MemoryStream _decompressed = new();
    private readonly ZLibStream _inflater;
    private readonly byte[] _buffer = new byte[16 * 1024];

    public ZlibStreamDecompressor()
    {
        _inflater = new ZLibStream(_compressed, CompressionMode.Decompress, leaveOpen: true);
    }

    /// <summary>
    /// Feeds a chunk of compressed data. Returns true and the decompressed message once a chunk ends
    /// with the sync-flush marker; the returned memory is valid until the next call.
    /// </summary>
    public bool TryDecompress(ReadOnlySpan<byte> chunk, out ReadOnlyMemory<byte> message)
    {
        _compressed.Seek(0, SeekOrigin.End);
        _compressed.Write(chunk);

        if (!chunk.EndsWith(SyncFlushSuffix))
        {
            message = default;
            return false;
        }

        _compressed.Position = 0;
        _decompressed.SetLength(0);
        int read;
        while ((read = _inflater.Read(_buffer)) > 0)
            _decompressed.Write(_buffer, 0, read);

        // The inflater has consumed all pending input; reuse the buffer for the next message.
        _compressed.SetLength(0);
        message = _decompressed.GetBuffer().AsMemory(0, (int)_decompressed.Length);
        return true;
    }

    public void Dispose()
    {
        _inflater.Dispose();
        _compressed.Dispose();
        _decompressed.Dispose();
    }
}
