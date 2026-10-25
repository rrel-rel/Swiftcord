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
