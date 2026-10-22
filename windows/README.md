# Swiftcord for Windows

A from-scratch Windows 11 Discord client in the spirit of Swiftcord: native (WinUI 3), fast, and light.

## Layout

| Project | Target | Purpose |
| --- | --- | --- |
| `src/Swiftcord.Core` | `net10.0` | Platform-independent client: REST, gateway, state cache, markdown, permissions. |
| `tests/Swiftcord.Core.Tests` | `net10.0` | Unit tests; run anywhere with `dotnet test`. |
| `src/Swiftcord.App` | `net10.0-windows` | WinUI 3 app *(next milestone; builds on Windows only)*. |

## Core overview

- **`DiscordClient`**: facade owning the REST client, gateway and state; start here.
- **`Rest/`**: `DiscordRestClient` with per-route rate limiting and 429 retries, plus multipart uploads.
- **`Gateway/`**: `GatewayClient` handles zlib-stream, heartbeats, zombie detection, resume/re-identify and fatal close codes. The transport is behind `IGatewayTransport` for testing.
- **`State/`**: `DiscordState` caches guilds, channels, members, messages, read state, typing and presence from gateway events, and computes the visible channel tree.
- **`Markdown/`**: Discord-flavored markdown → AST (bold/italic/underline/strike/spoiler, code, quotes, headings, subtext, lists, links, mentions, emoji, timestamps).
- **`Permissions/`**: Discord's permission resolution (roles + overwrites).

## Roadmap

1. ✅ Core library
2. WinUI 3 shell: login (WebView2), server rail, channel list, message list, composer
3. Polish: settings, toast notifications, `swiftcord://` protocol, auto-update (Velopack)
4. Voice and video

## Build

```sh
dotnet test                      # core + tests, any OS
```
