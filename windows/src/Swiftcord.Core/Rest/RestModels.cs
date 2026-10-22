using Swiftcord.Core.Models;

namespace Swiftcord.Core.Rest;

// Request bodies for REST endpoints.

public sealed class MessageCreateRequest
{
    public string? Content { get; set; }
    public string? Nonce { get; set; }
    public bool? Tts { get; set; }
    public MessageReference? MessageReference { get; set; }
    public AllowedMentions? AllowedMentions { get; set; }
    public List<Snowflake>? StickerIds { get; set; }
    public MessageFlags? Flags { get; set; }
    /// <summary>Filled in automatically from the uploaded files.</summary>
    public List<AttachmentRequest>? Attachments { get; set; }
}

public sealed class MessageEditRequest
{
    public string? Content { get; set; }
    public MessageFlags? Flags { get; set; }
}

public sealed class AllowedMentions
{
    /// <summary>Any of "roles", "users", "everyone".</summary>
    public List<string>? Parse { get; set; }
    public List<Snowflake>? Roles { get; set; }
    public List<Snowflake>? Users { get; set; }
    public bool? RepliedUser { get; set; }
}

public sealed class AttachmentRequest
{
    public int Id { get; set; }
    public string Filename { get; set; } = "";
    public string? Description { get; set; }
}

/// <summary>A file to upload with a message. The stream is read once and not disposed.</summary>
public sealed record FileUpload(string FileName, Stream Content, string? ContentType = null, string? Description = null);

internal sealed class CreateDmRequest
{
    public List<Snowflake> Recipients { get; set; } = [];
}

internal sealed class AckRequest
{
    public string? Token { get; set; }
}

internal sealed class LogoutRequest
{
    public string? Provider { get; set; }
    public string? VoipProvider { get; set; }
}
