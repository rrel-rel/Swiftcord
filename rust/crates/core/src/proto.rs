//! Reads the parts of Discord's protobuf user settings (PreloadedUserSettings) that the client uses.
//! Only the wire format is needed, so this avoids a protobuf dependency; unknown fields are skipped.

use base64::Engine;

use crate::models::GuildFolder;
use crate::Snowflake;

const GUILD_FOLDERS_FIELD: u64 = 14;

/// The server folders in display order, or `None` when the settings don't include them.
pub fn read_guild_folders(base64: &str) -> Option<Vec<GuildFolder>> {
    let bytes = base64::engine::general_purpose::STANDARD.decode(base64).ok()?;
    let mut folders = None;
    for (field, value) in fields(&bytes).ok()? {
        if field == GUILD_FOLDERS_FIELD {
            if let Value::Bytes(b) = value {
                folders = Some(read_folders(b).ok()?);
            }
        }
    }
    folders
}

enum Value<'a> {
    Int(u64),
    Bytes(&'a [u8]),
}

// message GuildFolders { repeated GuildFolder folders = 1; repeated fixed64 guild_positions = 2; }
fn read_folders(data: &[u8]) -> Result<Vec<GuildFolder>, ()> {
    let mut out = Vec::new();
    for (field, value) in fields(data)? {
        if let (1, Value::Bytes(b)) = (field, value) {
            out.push(read_folder(b)?);
        }
    }
    Ok(out)
}

// message GuildFolder { repeated fixed64 guild_ids = 1; Int64Value id = 2; StringValue name = 3; UInt64Value color = 4; }
fn read_folder(data: &[u8]) -> Result<GuildFolder, ()> {
    let mut f = GuildFolder::default();
    for (field, value) in fields(data)? {
        match (field, value) {
            (1, Value::Bytes(b)) if b.len() % 8 == 0 => {
                f.guild_ids.extend(b.chunks_exact(8).map(|c| Snowflake(u64::from_le_bytes(c.try_into().unwrap()))));
            }
            (1, Value::Int(v)) => f.guild_ids.push(Snowflake(v)),
            (2, Value::Bytes(b)) => f.id = Some(wrapped_int(b)? as i64),
            (3, Value::Bytes(b)) => f.name = Some(wrapped_string(b)?),
            (4, Value::Bytes(b)) => f.color = Some(wrapped_int(b)? as u32),
            _ => {}
        }
    }
    Ok(f)
}

fn wrapped_int(data: &[u8]) -> Result<u64, ()> {
    for (field, value) in fields(data)? {
        if let (1, Value::Int(v)) = (field, value) {
            return Ok(v);
        }
    }
    Ok(0)
}

fn wrapped_string(data: &[u8]) -> Result<String, ()> {
    for (field, value) in fields(data)? {
        if let (1, Value::Bytes(b)) = (field, value) {
            return Ok(String::from_utf8_lossy(b).into_owned());
        }
    }
    Ok(String::new())
}

fn fields(data: &[u8]) -> Result<Vec<(u64, Value<'_>)>, ()> {
    let mut out = Vec::new();
    let mut pos = 0;
    while pos < data.len() {
        let key = varint(data, &mut pos)?;
        let field = key >> 3;
        match key & 7 {
            0 => out.push((field, Value::Int(varint(data, &mut pos)?))),
            1 => {
                let b = data.get(pos..pos + 8).ok_or(())?;
                out.push((field, Value::Int(u64::from_le_bytes(b.try_into().unwrap()))));
                pos += 8;
            }
            2 => {
                let len = varint(data, &mut pos)? as usize;
                let b = data.get(pos..pos.checked_add(len).ok_or(())?).ok_or(())?;
                out.push((field, Value::Bytes(b)));
                pos += len;
            }
            5 => pos += 4,
            _ => return Err(()),
        }
    }
    Ok(out)
}

fn varint(data: &[u8], pos: &mut usize) -> Result<u64, ()> {
    let mut result = 0u64;
    for shift in (0..64).step_by(7) {
        let b = *data.get(*pos).ok_or(())?;
        *pos += 1;
        result |= u64::from(b & 0x7f) << shift;
        if b & 0x80 == 0 {
            return Ok(result);
        }
    }
    Err(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encode_varint(mut v: u64, out: &mut Vec<u8>) {
        loop {
            let b = (v & 0x7f) as u8;
            v >>= 7;
            if v == 0 {
                out.push(b);
                return;
            }
            out.push(b | 0x80);
        }
    }

    fn len_field(field: u64, body: &[u8], out: &mut Vec<u8>) {
        encode_varint(field << 3 | 2, out);
        encode_varint(body.len() as u64, out);
        out.extend_from_slice(body);
    }

    #[test]
    fn encodes_status_settings() {
        let bytes = base64::engine::general_purpose::STANDARD.decode(status_settings("dnd")).unwrap();
        // Field 11 { field 1 { field 1: "dnd" } }.
        assert_eq!(bytes, [0x5a, 7, 0x0a, 5, 0x0a, 3, b'd', b'n', b'd']);
    }

    /// Prints what a real account's settings say about status: `SWIFTCORD_PROTO_FILE` names a saved
    /// `GET users/@me/settings-proto/1` response.
    #[test]
    #[ignore]
    fn print_status_settings_from_file() {
        let path = std::env::var("SWIFTCORD_PROTO_FILE").unwrap();
        let json: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        println!("{:?}", read_status_settings(json["settings"].as_str().unwrap()));
    }

    #[test]
    fn reads_back_status_and_activity_sharing() {
        assert_eq!(read_status_settings(&status_settings("dnd")).unwrap().status.as_deref(), Some("dnd"));
        assert_eq!(read_status_settings(&status_settings("dnd")).unwrap().show_activity, None);
        assert_eq!(read_status_settings(&show_activity_settings(true)).unwrap().show_activity, Some(true));
        // Off is an empty wrapper (the default value isn't written), which still counts as set.
        assert_eq!(read_status_settings(&show_activity_settings(false)).unwrap().show_activity, Some(false));
        let bytes = base64::engine::general_purpose::STANDARD.decode(show_activity_settings(false)).unwrap();
        assert_eq!(bytes, [0x5a, 2, 0x1a, 0]);
    }

    #[test]
    fn reads_folders() {
        // Folder with two guilds, id 7 and name "Games".
        let mut ids = Vec::new();
        ids.extend_from_slice(&10u64.to_le_bytes());
        ids.extend_from_slice(&20u64.to_le_bytes());
        let mut folder = Vec::new();
        len_field(1, &ids, &mut folder);
        len_field(2, &[0x08, 7], &mut folder);
        let mut name = Vec::new();
        len_field(1, b"Games", &mut name);
        len_field(3, &name, &mut folder);
        let mut folders = Vec::new();
        len_field(1, &folder, &mut folders);
        let mut settings = vec![0x08, 0x01]; // An unrelated varint field first.
        len_field(GUILD_FOLDERS_FIELD, &folders, &mut settings);

        let b64 = base64::engine::general_purpose::STANDARD.encode(settings);
        let read = read_guild_folders(&b64).unwrap();
        assert_eq!(read.len(), 1);
        assert_eq!(read[0].id, Some(7));
        assert_eq!(read[0].name.as_deref(), Some("Games"));
        assert_eq!(read[0].guild_ids, vec![Snowflake(10), Snowflake(20)]);
    }
}

// MARK: Emoji frecency (FrecencyUserSettings, settings-proto type 2)
//
// message FrecencyUserSettings { ...; EmojiFrecency emoji_frecency = 6; ...; EmojiFrecency emoji_reaction_frecency = 13; }
// message EmojiFrecency { map<string, FrecencyItem> emojis = 1; }
// message FrecencyItem { uint32 total_uses = 1; repeated uint64 recent_uses = 2; int32 frecency = 3; int32 score = 4; }

const EMOJI_FRECENCY_FIELD: u64 = 6;
const EMOJI_REACTION_FRECENCY_FIELD: u64 = 13;

/// The emoji someone uses most for reactions (falling back to the ones they use in messages), most used
/// first: keys are a Unicode emoji's name or a custom emoji's ID, as Discord's client stores them.
pub fn read_reaction_frecency(base64: &str) -> Option<Vec<String>> {
    let bytes = base64::engine::general_purpose::STANDARD.decode(base64).ok()?;
    let read = |wanted: u64| -> Option<Vec<(String, i64)>> {
        let mut out = Vec::new();
        for (field, value) in fields(&bytes).ok()? {
            let (true, Value::Bytes(freq)) = (field == wanted, value) else { continue };
            for (f, entry) in fields(freq).ok()? {
                let (1, Value::Bytes(entry)) = (f, entry) else { continue };
                let (mut key, mut score, mut uses) = (None, 0i64, 0i64);
                for (k, v) in fields(entry).ok()? {
                    match (k, v) {
                        (1, Value::Bytes(b)) => key = std::str::from_utf8(b).ok().map(str::to_owned),
                        (2, Value::Bytes(item)) => {
                            for (ik, iv) in fields(item).ok()? {
                                match (ik, iv) {
                                    (1, Value::Int(n)) => uses = n as i64,
                                    // int32: negative scores arrive as 10-byte varints.
                                    (4, Value::Int(n)) => score = n as i32 as i64,
                                    _ => {}
                                }
                            }
                        }
                        _ => {}
                    }
                }
                if let Some(k) = key {
                    out.push((k, if score != 0 { score } else { uses }));
                }
            }
        }
        Some(out)
    };
    let mut list = read(EMOJI_REACTION_FRECENCY_FIELD).filter(|l| !l.is_empty()).or_else(|| read(EMOJI_FRECENCY_FIELD))?;
    list.sort_by_key(|(_, s)| -s);
    Some(list.into_iter().map(|(k, _)| k).collect())
}

// MARK: Favourite GIFs (FrecencyUserSettings, settings-proto type 2)
//
// message FrecencyUserSettings { ...; FavoriteGIFs favorite_gifs = 2; ... }
// message FavoriteGIFs { map<string, FavoriteGIF> gifs = 1; bool hide_tooltip = 2; }
// message FavoriteGIF { GIFType format = 1; string src = 2; uint32 width = 3; uint32 height = 4; uint32 order = 5; }

const FAVORITE_GIFS_FIELD: u64 = 2;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FavoriteGif {
    /// The link that's sent (the map key).
    pub url: String,
    /// The media itself.
    pub src: String,
    /// 1 = image (GIF/WebP), 2 = video (MP4).
    pub format: u32,
    pub width: u32,
    pub height: u32,
    /// Higher is more recently added.
    pub order: u32,
}

/// Favourite GIFs, most recently added first.
pub fn read_favorite_gifs(base64: &str) -> Option<Vec<FavoriteGif>> {
    let bytes = base64::engine::general_purpose::STANDARD.decode(base64).ok()?;
    let mut out = Vec::new();
    for (field, value) in fields(&bytes).ok()? {
        let (FAVORITE_GIFS_FIELD, Value::Bytes(favs)) = (field, value) else { continue };
        for (f, entry) in fields(favs).ok()? {
            let (1, Value::Bytes(entry)) = (f, entry) else { continue };
            let mut gif = FavoriteGif { url: String::new(), src: String::new(), format: 0, width: 0, height: 0, order: 0 };
            for (k, v) in fields(entry).ok()? {
                match (k, v) {
                    (1, Value::Bytes(b)) => gif.url = String::from_utf8_lossy(b).into_owned(),
                    (2, Value::Bytes(b)) => {
                        for (gk, gv) in fields(b).ok()? {
                            match (gk, gv) {
                                (1, Value::Int(v)) => gif.format = v as u32,
                                (2, Value::Bytes(s)) => gif.src = String::from_utf8_lossy(s).into_owned(),
                                (3, Value::Int(v)) => gif.width = v as u32,
                                (4, Value::Int(v)) => gif.height = v as u32,
                                (5, Value::Int(v)) => gif.order = v as u32,
                                _ => {}
                            }
                        }
                    }
                    _ => {}
                }
            }
            if !gif.url.is_empty() {
                out.push(gif);
            }
        }
    }
    out.sort_by_key(|g| std::cmp::Reverse(g.order));
    Some(out)
}

/// Adds (`Some`) or removes (by `url`) a favourite GIF, returning the new settings as base64. Every other field
/// is copied byte for byte, since the whole message is written back.
pub fn edit_favorite_gifs(base64: &str, url: &str, add: Option<&FavoriteGif>) -> Option<String> {
    let engine = base64::engine::general_purpose::STANDARD;
    let bytes = engine.decode(base64).ok()?;
    let mut out = Vec::with_capacity(bytes.len() + 128);
    let mut favs: Vec<u8> = Vec::new();
    for (field, record, value) in raw_fields(&bytes).ok()? {
        if field != FAVORITE_GIFS_FIELD {
            out.extend_from_slice(record);
            continue;
        }
        // Keep existing entries except the one being changed, and any other subfields (e.g. hide_tooltip).
        for (f, rec, v) in raw_fields(value).ok()? {
            if f == 1 {
                let key = fields(v).ok()?.into_iter().find_map(|(k, val)| match (k, val) {
                    (1, Value::Bytes(b)) => Some(b),
                    _ => None,
                });
                if key == Some(url.as_bytes()) {
                    continue;
                }
            }
            favs.extend_from_slice(rec);
        }
    }
    if let Some(g) = add {
        let mut gif = Vec::new();
        put_varint_field(&mut gif, 1, g.format as u64);
        put_bytes_field(&mut gif, 2, g.src.as_bytes());
        put_varint_field(&mut gif, 3, g.width as u64);
        put_varint_field(&mut gif, 4, g.height as u64);
        put_varint_field(&mut gif, 5, g.order as u64);
        let mut entry = Vec::new();
        put_bytes_field(&mut entry, 1, url.as_bytes());
        put_bytes_field(&mut entry, 2, &gif);
        put_bytes_field(&mut favs, 1, &entry);
    }
    put_bytes_field(&mut out, FAVORITE_GIFS_FIELD, &favs);
    Some(engine.encode(out))
}

const STATUS_FIELD: u64 = 11;

/// User settings holding only a status (`status { status: StringValue }`), base64-encoded, for a partial update.
pub fn status_settings(status: &str) -> String {
    let mut value = Vec::new();
    put_bytes_field(&mut value, 1, status.as_bytes());
    let mut settings = Vec::new();
    put_bytes_field(&mut settings, 1, &value);
    let mut out = Vec::new();
    put_bytes_field(&mut out, STATUS_FIELD, &settings);
    base64::engine::general_purpose::STANDARD.encode(out)
}

/// The status part of the user settings, as far as it's set: the chosen status ("online", "idle", "dnd",
/// "invisible") and whether activities are shared ("Share my activity").
#[derive(Debug, Default, Clone, PartialEq)]
pub struct StatusSettings {
    pub status: Option<String>,
    pub show_activity: Option<bool>,
}

// message StatusSettings { StringValue status = 1; CustomStatus custom_status = 2; BoolValue show_current_game = 3; }
pub fn read_status_settings(base64: &str) -> Option<StatusSettings> {
    let bytes = base64::engine::general_purpose::STANDARD.decode(base64).ok()?;
    let mut out = StatusSettings::default();
    for (field, value) in fields(&bytes).ok()? {
        let (STATUS_FIELD, Value::Bytes(status)) = (field, value) else { continue };
        for (field, value) in fields(status).ok()? {
            match (field, value) {
                // Wrapper messages: the value is their field 1, absent when it's the default.
                (1, Value::Bytes(w)) => {
                    let text = fields(w).ok()?.into_iter().find_map(|(f, v)| if let (1, Value::Bytes(s)) = (f, v) { Some(s) } else { None });
                    out.status = Some(String::from_utf8_lossy(text.unwrap_or_default()).into_owned());
                }
                (3, Value::Bytes(w)) => {
                    out.show_activity = Some(fields(w).ok()?.into_iter().any(|(f, v)| matches!((f, v), (1, Value::Int(1)))));
                }
                _ => {}
            }
        }
    }
    Some(out)
}

/// User settings holding only "Share my activity" (`status { show_current_game: BoolValue }`), for a partial update.
pub fn show_activity_settings(show: bool) -> String {
    let mut value = Vec::new();
    if show {
        put_varint_field(&mut value, 1, 1);
    }
    let mut settings = Vec::new();
    put_bytes_field(&mut settings, 3, &value);
    let mut out = Vec::new();
    put_bytes_field(&mut out, STATUS_FIELD, &settings);
    base64::engine::general_purpose::STANDARD.encode(out)
}

/// Top-level fields as (field number, the whole record's bytes, the value's bytes).
fn raw_fields(data: &[u8]) -> Result<Vec<(u64, &[u8], &[u8])>, ()> {
    let mut out = Vec::new();
    let mut pos = 0;
    while pos < data.len() {
        let start = pos;
        let key = varint(data, &mut pos)?;
        let value_start;
        match key & 7 {
            0 => {
                value_start = pos;
                varint(data, &mut pos)?;
            }
            1 => {
                value_start = pos;
                pos += 8;
            }
            2 => {
                let len = varint(data, &mut pos)? as usize;
                value_start = pos;
                pos = pos.checked_add(len).ok_or(())?;
            }
            5 => {
                value_start = pos;
                pos += 4;
            }
            _ => return Err(()),
        }
        if pos > data.len() {
            return Err(());
        }
        out.push((key >> 3, &data[start..pos], &data[value_start..pos]));
    }
    Ok(out)
}

fn put_varint(out: &mut Vec<u8>, mut v: u64) {
    loop {
        let b = (v & 0x7f) as u8;
        v >>= 7;
        if v == 0 {
            out.push(b);
            return;
        }
        out.push(b | 0x80);
    }
}

fn put_varint_field(out: &mut Vec<u8>, field: u64, v: u64) {
    put_varint(out, field << 3);
    put_varint(out, v);
}

fn put_bytes_field(out: &mut Vec<u8>, field: u64, b: &[u8]) {
    put_varint(out, field << 3 | 2);
    put_varint(out, b.len() as u64);
    out.extend_from_slice(b);
}


/// Whether editing these settings would leave everything else intact: a no-op edit (removing a URL that isn't
/// there) must keep every field but favourites byte for byte, and the favourites themselves unchanged.
pub fn edit_round_trips(base64: &str) -> bool {
    let engine = base64::engine::general_purpose::STANDARD;
    let Some(edited) = edit_favorite_gifs(base64, "\u{0}swiftcord-no-such-gif", None) else { return false };
    let (Ok(before), Ok(after)) = (engine.decode(base64), engine.decode(&edited)) else { return false };
    let others = |b: &[u8]| -> Option<Vec<Vec<u8>>> {
        Some(raw_fields(b).ok()?.into_iter().filter(|f| f.0 != FAVORITE_GIFS_FIELD).map(|f| f.1.to_vec()).collect())
    };
    let favs_before = read_favorite_gifs(base64).map(|mut v| {
        v.sort_by(|a, b| a.url.cmp(&b.url));
        v
    });
    let favs_after = read_favorite_gifs(&edited).map(|mut v| {
        v.sort_by(|a, b| a.url.cmp(&b.url));
        v
    });
    others(&before).is_some() && others(&before) == others(&after) && favs_before.is_some() && favs_before == favs_after
}

#[cfg(test)]
mod favorite_tests {
    use super::*;

    /// Prints the most used reaction emoji from a real settings blob in `SWIFTCORD_TEST_FRECENCY` (a file of
    /// base64), to check the key format.
    #[test]
    #[ignore]
    fn reads_real_reaction_frecency() {
        let path = std::env::var("SWIFTCORD_TEST_FRECENCY").expect("SWIFTCORD_TEST_FRECENCY");
        let b64 = std::fs::read_to_string(path).unwrap();
        let top = read_reaction_frecency(b64.trim()).expect("parses");
        println!("{} keys, top: {:?}", top.len(), &top[..top.len().min(12)]);
        assert!(!top.is_empty());
    }

    #[test]
    fn reaction_frecency_orders_by_score() {
        // Two entries: "joy" (score 50) and custom emoji "123" (score 90), in field 13.
        let item = |uses: u8, score: u8| vec![0x08, uses, 0x20, score];
        let mut entries = Vec::new();
        for (key, it) in [("joy".as_bytes(), item(3, 50)), (b"123".as_slice(), item(1, 90))] {
            let mut entry = Vec::new();
            put_bytes_field(&mut entry, 1, key);
            put_bytes_field(&mut entry, 2, &it);
            put_bytes_field(&mut entries, 1, &entry);
        }
        let mut settings = Vec::new();
        put_bytes_field(&mut settings, 13, &entries);
        let b64 = base64::engine::general_purpose::STANDARD.encode(&settings);
        assert_eq!(read_reaction_frecency(&b64), Some(vec!["123".to_owned(), "joy".to_owned()]));
    }

    fn gif(url: &str, order: u32) -> FavoriteGif {
        FavoriteGif { url: url.into(), src: format!("{url}.mp4"), format: 2, width: 320, height: 240, order }
    }

    #[test]
    fn adds_and_removes_while_keeping_other_fields() {
        let engine = base64::engine::general_purpose::STANDARD;
        // Field 1 (versions) and field 5 (favourite emoji) must survive untouched.
        let mut original = Vec::new();
        put_bytes_field(&mut original, 1, &[0x08, 0x07]);
        put_bytes_field(&mut original, 5, b"emoji-data");
        let b64 = engine.encode(&original);

        let one = edit_favorite_gifs(&b64, "https://a", Some(&gif("https://a", 1))).unwrap();
        let two = edit_favorite_gifs(&one, "https://b", Some(&gif("https://b", 2))).unwrap();
        assert_eq!(read_favorite_gifs(&two).unwrap(), vec![gif("https://b", 2), gif("https://a", 1)]);

        let back = edit_favorite_gifs(&two, "https://b", None).unwrap();
        assert_eq!(read_favorite_gifs(&back).unwrap(), vec![gif("https://a", 1)]);

        // The other fields are byte-for-byte the same, in order.
        let bytes = engine.decode(&back).unwrap();
        let others: Vec<Vec<u8>> = raw_fields(&bytes).unwrap().into_iter().filter(|f| f.0 != 2).map(|f| f.1.to_vec()).collect();
        let orig: Vec<Vec<u8>> = raw_fields(&original).unwrap().into_iter().map(|f| f.1.to_vec()).collect();
        assert_eq!(others, orig);
        assert!(edit_round_trips(&two));
        assert!(edit_round_trips(&b64));
    }

    #[test]
    fn re_adding_replaces_rather_than_duplicates() {
        let b64 = edit_favorite_gifs("", "https://a", Some(&gif("https://a", 1))).unwrap();
        let b64 = edit_favorite_gifs(&b64, "https://a", Some(&gif("https://a", 5))).unwrap();
        assert_eq!(read_favorite_gifs(&b64).unwrap(), vec![gif("https://a", 5)]);
    }
}
