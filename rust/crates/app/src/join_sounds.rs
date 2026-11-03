//! Custom join sounds: someone with one set (a Nitro feature of the official client) plays it as they join a
//! voice channel, and Discord tells everyone there with VOICE_CHANNEL_EFFECT_SEND. Sounds are fetched from the
//! CDN once and kept for the session.

use std::sync::Arc;

use crate::app::{App, AppEvent};

/// Soundboard sounds are at most 5.2 seconds and 512 KiB; these allow for a little more.
const MAX_BYTES: usize = 2 * 1024 * 1024;
const MAX_SAMPLES: usize = 48_000 * 2 * 15;

/// Plays a sound, fetching it first if this is the first time it's come up.
pub fn play(app: &mut App, id: String, volume: f32) {
    // It goes into a URL and a file name.
    if id.is_empty() || !id.bytes().all(|b| b.is_ascii_digit()) {
        return;
    }
    if let Some(pcm) = app.sounds.clip(&id) {
        app.sounds.play_clip(pcm, volume, &app.settings.output_device);
        return;
    }
    app.spawn(async move {
        let result = fetch(&id).await;
        if let Err(e) = &result {
            tracing::info!("join sound {id}: {e}");
        }
        AppEvent::JoinSound { id, volume, pcm: result.ok().map(Arc::new) }
    });
}

async fn fetch(id: &str) -> Result<Vec<f32>, String> {
    let response = reqwest::get(format!("https://cdn.discordapp.com/soundboard-sounds/{id}")).await.map_err(|e| e.to_string())?;
    let bytes = response.error_for_status().map_err(|e| e.to_string())?.bytes().await.map_err(|e| e.to_string())?;
    if bytes.len() > MAX_BYTES {
        return Err(format!("too large ({} bytes)", bytes.len()));
    }
    let id = id.to_owned();
    tokio::task::spawn_blocking(move || decode(&id, &bytes)).await.map_err(|e| e.to_string())?
}

/// To 48 kHz stereo. They're Ogg or MP3.
fn decode(id: &str, bytes: &[u8]) -> Result<Vec<f32>, String> {
    if bytes.starts_with(b"OggS") {
        if let Ok(pcm) = swiftcord_voice::ogg::decode_ogg_opus(bytes) {
            return Ok(pcm);
        }
    }
    // Media Foundation reads files, and goes by the name for what kind.
    let path = crate::platform::data_dir().join(format!("join-sound-{id}.mp3"));
    std::fs::write(&path, bytes).map_err(|e| e.to_string())?;
    let pcm = media_foundation(&path);
    let _ = std::fs::remove_file(&path);
    pcm
}

#[cfg(windows)]
fn media_foundation(path: &std::path::Path) -> Result<Vec<f32>, String> {
    use windows::core::HSTRING;
    use windows::Win32::Media::MediaFoundation::*;
    use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};

    use crate::player::decode::{pcm, select_audio};
    use crate::player::Converter;

    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        MFStartup(MF_VERSION, MFSTARTUP_LITE).map_err(|e| e.to_string())?;
    }
    let reader = unsafe { MFCreateSourceReaderFromURL(&HSTRING::from(path.as_os_str()), None) }.map_err(|e| e.to_string())?;
    unsafe {
        let _ = reader.SetStreamSelection(MF_SOURCE_READER_ALL_STREAMS.0 as u32, false);
    }
    let (rate, channels) = select_audio(&reader).map_err(|e| e.to_string())?;
    let mut conv = Converter::new(rate, channels);
    let mut out = Vec::new();
    while out.len() < MAX_SAMPLES {
        let (mut flags, mut sample) = (0u32, None);
        unsafe { reader.ReadSample(MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32, 0, None, Some(&mut flags), None, Some(&mut sample)) }.map_err(|e| e.to_string())?;
        if flags & MF_SOURCE_READERF_ERROR.0 as u32 != 0 {
            return Err("the sound stopped decoding".into());
        }
        if let Some(sample) = sample {
            out.extend(conv.convert(&pcm(&sample).map_err(|e| e.to_string())?));
        }
        if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
            break;
        }
    }
    out.truncate(MAX_SAMPLES);
    Ok(out)
}

#[cfg(not(windows))]
fn media_foundation(_: &std::path::Path) -> Result<Vec<f32>, String> {
    Err("decoding needs Windows".into())
}
