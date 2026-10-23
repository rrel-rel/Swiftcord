# Swiftcord for Windows

A from-scratch Windows 11 Discord client in the spirit of Swiftcord: native (WinUI 3), fast, and light.

## Layout

| Project | Target | Purpose |
| --- | --- | --- |
| `src/Swiftcord.Core` | `net10.0` | Platform-independent client: REST, gateway, state cache, markdown, permissions. |
| `src/Swiftcord.ViewModels` | `net10.0` | All app logic (MVVM): navigation, chat, composer, login. No UI framework dependency. |
| `src/Swiftcord.App` | `net10.0-windows` | WinUI 3 app: XAML views, markdown renderer, Windows services. Builds on Windows only. |
| `tests/*` | `net10.0` | Unit tests for Core and ViewModels; run anywhere. |

## Core overview

- **`DiscordClient`**: facade owning the REST client, gateway and state; start here.
- **`Rest/`**: `DiscordRestClient` with per-route rate limiting and 429 retries, plus multipart uploads.
- **`Gateway/`**: `GatewayClient` handles zlib-stream, heartbeats, zombie detection, resume/re-identify and fatal close codes. The transport is behind `IGatewayTransport` for testing.
- **`State/`**: `DiscordState` caches guilds, channels, members, messages, read state, typing and presence from gateway events, and computes the visible channel tree.
- **`Markdown/`**: Discord-flavored markdown → AST (bold/italic/underline/strike/spoiler, code, quotes, headings, subtext, lists, links, mentions, emoji, timestamps).
- **`Permissions/`**: Discord's permission resolution (roles + overwrites).

## App overview

- **Login:** the real discord.com login page in WebView2 (so captcha, 2FA and passkeys work); the token is captured from its API requests, verified, and stored in Windows Credential Manager. A manual token option (user or bot) is also available.
- **Shell:** Mica window; server rail with unread pills and mention badges; channel sidebar with collapsible categories, permission-filtered channels, and DMs with presence; user panel with log out.
- **Chat:** virtualized message list that stays pinned to the bottom and loads history as you scroll up; grouping, date dividers, replies, mentions highlighting, embeds, attachments, stickers, reactions; context menu (react, reply, edit, copy, delete).
- **Composer:** Enter to send, Shift+Enter for newline, Up to edit your last message, Esc to cancel; replies with mention toggle; file picker and drag-and-drop uploads; optimistic sending with retry; typing indicators.

## Roadmap

1. ✅ Core library
2. ✅ WinUI 3 app: login, server rail, channel list, message list, composer
3. ✅ Polish: settings, toast notifications, `swiftcord://` links, auto-update (Velopack)
4. Voice and video

## Build & run

**Windows 11** (Visual Studio 2022+ with the *WinUI application development* workload, or the .NET 10 SDK):

```powershell
dotnet run --project src/Swiftcord.App          # or open Swiftcord.slnx and press F5
```

The app is unpackaged and self-contained, so it runs straight from `bin/` without installing an MSIX.

**Any OS** (core and view-model tests):

```sh
dotnet test Swiftcord.CrossPlatform.slnf
```

## Links

`swiftcord://channels/{server id or @me}/{channel id}` opens a conversation, as do `https://discord.com/channels/…`
links clicked inside the app. The scheme is registered for the current user each time Swiftcord starts. Only one
copy of the app runs; a second launch (or a link) is handed to the open window.

## Releases and updates

Installed copies update themselves from GitHub Releases using [Velopack](https://velopack.io). To build installers
(`Swiftcord.Windows-win-x64-Setup.exe` and the ARM64 equivalent) and update packages:

```powershell
./build/release.ps1 -Version 0.2.0            # artifacts/releases
./build/release.ps1 -Version 0.2.0 -Upload    # also publish to GitHub Releases (needs $env:GITHUB_TOKEN)
```

Copies run from a build folder (`dotnet run`) don't update; Settings → Updates says so.

## Logs

Swiftcord writes `%LOCALAPPDATA%\Swiftcord\Logs\swiftcord.log` (the previous run is kept as `swiftcord.old.log`).
Settings → Troubleshooting → Open logs folder gets you there.
