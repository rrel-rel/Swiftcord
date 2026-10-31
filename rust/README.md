# '08cord for Windows (Rust)

A lightweight native Discord client for Windows 11, written in Rust with [egui](https://github.com/emilk/egui).

## Layout

| Crate | Purpose |
| --- | --- |
| `crates/core` | Platform-independent client: gateway (zlib-stream, heartbeats, resume), REST with rate limiting, state cache, Discord markdown, permissions, QR-code login. |
| `crates/voice` | Voice: voice gateway v8 with DAVE end-to-end encryption ([davey](https://crates.io/crates/davey)), AES-GCM / XChaCha20 RTP encryption, Opus, WASAPI audio. |
| `crates/app` | The egui app: login, server rail, channels, chat, composer, voice controls, settings. |

## Features

- **Sign in** with a QR code (Discord phone app), the real discord.com login page (WebView2, in a child process so
  the client itself never loads it), or a token. The token is kept in Windows Credential Manager.
- **Messaging:** servers and folders, channel categories filtered by permissions, DMs and group DMs, unread and
  mention badges, a virtualized history that loads as you scroll up, markdown (styles, spoilers, code, quotes,
  headings, lists, mentions, custom and Unicode emoji, timestamps, links), embeds, image/video/file attachments,
  stickers, reactions, replies, editing (Up arrow), deleting, pins, uploads (picker or drag and drop), typing
  indicators, and Windows toast notifications that follow your Discord notification settings.
- **Voice:** join server voice channels and DM calls, end-to-end encrypted (DAVE) when the call is; mute, deafen,
  voice-activity sensitivity, input and output device selection, speaking indicators.

## Build

Needs Rust (stable) and the MSVC build tools with the Windows SDK. There are no other native dependencies: Opus is
the transpiled [unsafe-libopus](https://crates.io/crates/unsafe-libopus), and the rest is Windows' own: Direct3D 11
for drawing, WIC for images, SChannel for TLS, CNG for QR login's key, the clipboard, and fonts and icons.

```powershell
cargo run -p swiftcord              # debug
cargo build --release -p swiftcord  # target\release\swiftcord.exe
cargo test --workspace --exclude swiftcord
cargo run -p swiftcord-voice --example devices   # check audio devices
```

## Data

`%LOCALAPPDATA%\Swiftcord`: `logs\swiftcord.log` (`SWIFTCORD_LOG=debug` for more), `settings.json`,
`cache\media` (avatars, icons and emoji), `recent-emoji.json`.

## Notes

'08cord is an unofficial client and uses your user token; that's against Discord's Terms of Service, so use it
at your own risk.
