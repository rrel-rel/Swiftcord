using System.IO.Compression;
using System.Text;
using Swiftcord.Core.Gateway;

namespace Swiftcord.Core.Tests.Gateway;

public sealed class ZlibStreamDecompressorTests
{
    /// <summary>Produces zlib-stream frames the way Discord does: one shared context, sync-flushed per message.</summary>
    private static List<byte[]> Compress(params string[] messages)
    {
        var output = new MemoryStream();
        using var zlib = new ZLibStream(output, CompressionLevel.Optimal, leaveOpen: true);
        var frames = new List<byte[]>();
        foreach (var message in messages)
        {
            var start = output.Length;
            zlib.Write(Encoding.UTF8.GetBytes(message));
            zlib.Flush(); // Z_SYNC_FLUSH: ends with 00 00 FF FF.
            frames.Add(output.ToArray()[(int)start..]);
        }
        return frames;
    }

    [Fact]
    public void Decompresses_consecutive_messages_sharing_one_context()
    {
        var messages = new[] { """{"op":10}""", """{"op":0,"t":"READY"}""", new string('x', 100_000) };
        var frames = Compress(messages);
        using var decompressor = new ZlibStreamDecompressor();

        for (var i = 0; i < messages.Length; i++)
        {
            Assert.True(decompressor.TryDecompress(frames[i], out var result));
            Assert.Equal(messages[i], Encoding.UTF8.GetString(result.Span));
        }
    }

    [Fact]
    public void Buffers_a_message_split_across_chunks_until_the_flush_marker()
    {
        var frame = Compress("""{"op":0,"d":{"content":"hello"}}""")[0];
        using var decompressor = new ZlibStreamDecompressor();

        Assert.False(decompressor.TryDecompress(frame.AsSpan(0, 5), out _));
        Assert.True(decompressor.TryDecompress(frame.AsSpan(5), out var result));
        Assert.Equal("""{"op":0,"d":{"content":"hello"}}""", Encoding.UTF8.GetString(result.Span));
    }
}
