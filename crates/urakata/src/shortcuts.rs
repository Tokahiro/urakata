//! Steam's `shortcuts.vdf` (non-Steam games in the library): read, change
//! and write it without Windows calls, so the rules have unit tests.
//!
//! The file is binary KeyValues: a type byte (`0x00` map, `0x01` string,
//! `0x02` 32-bit integer, `0x08` end of map), a key with a zero byte at the
//! end, then the value. The root map is `shortcuts`; its entries are maps
//! with the keys `"0"`, `"1"`, ... (Valve Developer Community, "Steam
//! Library Shortcuts"). Other type bytes exist in other binary KeyValues
//! files; this module refuses them, so it never writes back a file it did
//! not fully read.

const MAP: u8 = 0x00;
const STRING: u8 = 0x01;
const INT: u8 = 0x02;
const END: u8 = 0x08;

/// The library name of the entry.
pub const APP_NAME: &str = "urakata";

/// The program the entry starts. An entry counts as ours when the file name
/// of its `Exe` is this name, so a move of the install folder or a rename of
/// the entry in Steam does not make a second entry.
pub const APP_EXE: &str = "urakata-app.exe";

/// One value in the tree. Strings stay bytes, so a file with text that is
/// not valid UTF-8 is written back unchanged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    Map(Vec<(Vec<u8>, Value)>),
    Str(Vec<u8>),
    Int(u32),
}

/// The list of non-Steam games: the entries of the `shortcuts` map, in order.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Shortcuts {
    pub entries: Vec<Value>,
}

/// What `urakata steam-entry` does. Here, not in the Windows-only module,
/// so the command parser and its tests build on every platform.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryAction {
    Status,
    Add,
    Remove,
}

/// What `add` did.
#[derive(Debug, PartialEq, Eq)]
pub enum Added {
    /// A new entry was appended.
    New,
    /// The entry was there, and `add` changed these fields.
    Updated(Changes),
    /// The entry was there and correct. Nothing to write.
    Unchanged,
}

/// The fields that `add` changed in existing entries.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Changes {
    /// `Exe` or `StartDir`: the program moved.
    pub path: bool,
    /// Empty `LaunchOptions` got [`LAUNCH_OPTIONS`].
    pub options: bool,
    /// An entry without `appid` got one.
    pub app_id: bool,
}

impl Changes {
    /// Text for `urakata steam-entry --add`, for example
    /// "entry updated: launch options".
    pub fn text(self) -> String {
        let mut what = Vec::new();
        if self.path {
            what.push("new program path");
        }
        if self.options {
            what.push("launch options");
        }
        if self.app_id {
            what.push("appid");
        }
        format!("entry updated: {}", what.join(", "))
    }
}

/// Reads a `shortcuts.vdf`. An empty file is an empty list (Steam can leave
/// one), not an error.
pub fn parse(bytes: &[u8]) -> Result<Shortcuts, String> {
    if bytes.is_empty() {
        return Ok(Shortcuts::default());
    }
    let mut pos = 0;
    let kind = take_byte(bytes, &mut pos)?;
    let key = take_cstr(bytes, &mut pos)?;
    if kind != MAP || !key.eq_ignore_ascii_case(b"shortcuts") {
        return Err("shortcuts.vdf: the root is not the map 'shortcuts'".into());
    }
    let root = take_map(bytes, &mut pos, 0)?;
    // The root map is closed by one more end byte.
    if take_byte(bytes, &mut pos)? != END || pos != bytes.len() {
        return Err("shortcuts.vdf: unexpected data after the root map".into());
    }
    let mut entries = Vec::with_capacity(root.len());
    for (_, value) in root {
        match value {
            Value::Map(_) => entries.push(value),
            _ => return Err("shortcuts.vdf: an entry is not a map".into()),
        }
    }
    Ok(Shortcuts { entries })
}

/// Writes the list. Entry keys are numbered again from `"0"`, as Steam does.
pub fn serialize(list: &Shortcuts) -> Vec<u8> {
    let mut out = vec![MAP];
    out.extend_from_slice(b"shortcuts\0");
    for (i, entry) in list.entries.iter().enumerate() {
        put(&mut out, i.to_string().as_bytes(), entry);
    }
    out.push(END);
    out.push(END);
    out
}

/// Launch options of our entry: the settings window opens full screen and
/// takes a controller (it is started from Big Picture).
pub const LAUNCH_OPTIONS: &str = "--big-picture";

impl Shortcuts {
    /// The index of our entry, if there is one.
    pub fn find(&self) -> Option<usize> {
        self.entries.iter().position(is_ours)
    }

    /// Adds our entry for the program at `exe` (full path, no quotes), or
    /// changes the path of the entries that are there already (all of them,
    /// if the user added more than one). An existing entry with no launch
    /// options gets [`LAUNCH_OPTIONS`]; its other fields (name, options the
    /// user typed, tags) stay as the user set them.
    pub fn add(&mut self, exe: &str) -> Added {
        let quoted_exe = format!("\"{exe}\"");
        let quoted_dir = format!("\"{}\"", parent(exe));
        if self.find().is_some() {
            let mut changed = Changes::default();
            for i in 0..self.entries.len() {
                if !is_ours(&self.entries[i]) {
                    continue;
                }
                // The artwork file names need an appid. An entry from an
                // old Steam or another tool may have none.
                if !matches!(field(&self.entries[i], b"appid"), Some(Value::Int(_))) {
                    let name = match field(&self.entries[i], b"AppName") {
                        Some(Value::Str(n)) => String::from_utf8_lossy(n).into_owned(),
                        _ => APP_NAME.into(),
                    };
                    let id = self.free_app_id(app_id(&name, &quoted_exe));
                    let Value::Map(fields) = &mut self.entries[i] else {
                        unreachable!("is_ours matches maps only");
                    };
                    set_value(fields, b"appid", Value::Int(id));
                    changed.app_id = true;
                }
                let Value::Map(fields) = &mut self.entries[i] else {
                    unreachable!("is_ours matches maps only");
                };
                changed.path |= set_str(fields, b"Exe", quoted_exe.as_bytes());
                changed.path |= set_str(fields, b"StartDir", quoted_dir.as_bytes());
                let no_options = !fields.iter().any(|(k, v)| {
                    k.eq_ignore_ascii_case(b"LaunchOptions")
                        && !matches!(v, Value::Str(o) if o.trim_ascii().is_empty())
                });
                if no_options {
                    changed.options |= set_str(fields, b"LaunchOptions", LAUNCH_OPTIONS.as_bytes());
                }
            }
            return if changed != Changes::default() {
                Added::Updated(changed)
            } else {
                Added::Unchanged
            };
        }
        let id = self.free_app_id(app_id(APP_NAME, &quoted_exe));
        self.entries
            .push(new_entry(id, APP_NAME, &quoted_exe, &quoted_dir));
        Added::New
    }

    /// The `appid` of each of our entries, without repeats. Steam names the
    /// entry's artwork files by it.
    pub fn app_ids(&self) -> Vec<u32> {
        let mut ids = Vec::new();
        for entry in self.entries.iter().filter(|e| is_ours(e)) {
            if let Some(Value::Int(id)) = field(entry, b"appid")
                && !ids.contains(id)
            {
                ids.push(*id);
            }
        }
        ids
    }

    /// Points the `icon` field of our entries to `path_for(appid)`, unless
    /// the user chose another icon in Steam. Returns whether a field
    /// changed.
    pub fn set_icons(&mut self, path_for: impl Fn(u32) -> String) -> bool {
        let mut changed = false;
        for entry in self.entries.iter_mut().filter(|e| is_ours(e)) {
            let Some(Value::Int(id)) = field(entry, b"appid") else {
                continue;
            };
            let id = *id;
            let ours = match field(entry, b"icon") {
                Some(Value::Str(v)) => {
                    let v = String::from_utf8_lossy(v);
                    v.trim().is_empty() || crate::steam_art::is_our_icon(v.trim(), id)
                }
                Some(_) => false,
                None => true,
            };
            if ours {
                let Value::Map(fields) = entry else {
                    unreachable!("is_ours matches maps only");
                };
                changed |= set_str(fields, b"icon", path_for(id).as_bytes());
            }
        }
        changed
    }

    /// Removes our entries. Returns how many were removed.
    pub fn remove(&mut self) -> usize {
        let before = self.entries.len();
        self.entries.retain(|e| !is_ours(e));
        before - self.entries.len()
    }

    /// `wanted`, or the next free ID after it when another entry uses it.
    fn free_app_id(&self, wanted: u32) -> u32 {
        let used: Vec<u32> = self
            .entries
            .iter()
            .filter_map(|e| match field(e, b"appid") {
                Some(Value::Int(v)) => Some(*v),
                _ => None,
            })
            .collect();
        let mut id = wanted;
        while used.contains(&id) {
            id = id.wrapping_add(1) | 0x8000_0000;
        }
        id
    }
}

/// The ID Valve's page gives for a shortcut: CRC-32 of name, program and a
/// zero byte, with the top bit set. Steam uses it for the library images.
/// Only uniqueness matters for urakata; `free_app_id` makes sure of that.
pub fn app_id(name: &str, quoted_exe: &str) -> u32 {
    let mut data = Vec::with_capacity(name.len() + quoted_exe.len() + 1);
    data.extend_from_slice(name.as_bytes());
    data.extend_from_slice(quoted_exe.as_bytes());
    data.push(0);
    crc32(&data) | 0x8000_0000
}

pub(crate) fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc ^= u32::from(b);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

/// The fields in the order Valve's page writes them.
fn new_entry(id: u32, name: &str, quoted_exe: &str, quoted_dir: &str) -> Value {
    let s = |k: &str, v: &str| (k.as_bytes().to_vec(), Value::Str(v.as_bytes().to_vec()));
    let i = |k: &str, v: u32| (k.as_bytes().to_vec(), Value::Int(v));
    Value::Map(vec![
        i("appid", id),
        s("AppName", name),
        s("Exe", quoted_exe),
        s("StartDir", quoted_dir),
        s("icon", ""),
        s("ShortcutPath", ""),
        s("LaunchOptions", LAUNCH_OPTIONS),
        i("IsHidden", 0),
        i("AllowDesktopConfig", 1),
        i("AllowOverlay", 1),
        i("OpenVR", 0),
        i("Devkit", 0),
        s("DevkitGameID", ""),
        i("DevkitOverrideAppID", 0),
        i("LastPlayTime", 0),
        s("FlatpakAppID", ""),
        s("sortas", ""),
        (b"tags".to_vec(), Value::Map(Vec::new())),
    ])
}

fn is_ours(entry: &Value) -> bool {
    let Some(Value::Str(exe)) = field(entry, b"Exe") else {
        return false;
    };
    let exe = String::from_utf8_lossy(exe);
    let exe = exe.trim().trim_matches('"');
    let name = exe.rsplit(['\\', '/']).next().unwrap_or(exe);
    name.eq_ignore_ascii_case(APP_EXE)
}

/// Steam writes field names in mixed case (`icon`, `AppName`) and older
/// files differ; compare without case.
fn field<'a>(entry: &'a Value, key: &[u8]) -> Option<&'a Value> {
    let Value::Map(fields) = entry else {
        return None;
    };
    fields
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(key))
        .map(|(_, v)| v)
}

/// Sets a string field. Returns whether it changed.
fn set_str(fields: &mut Vec<(Vec<u8>, Value)>, key: &[u8], value: &[u8]) -> bool {
    set_value(fields, key, Value::Str(value.to_vec()))
}

/// Sets a field. Returns whether it changed.
fn set_value(fields: &mut Vec<(Vec<u8>, Value)>, key: &[u8], new: Value) -> bool {
    match fields.iter_mut().find(|(k, _)| k.eq_ignore_ascii_case(key)) {
        Some((_, v)) if *v == new => false,
        Some((_, v)) => {
            *v = new;
            true
        }
        None => {
            fields.push((key.to_vec(), new));
            true
        }
    }
}

/// The folder of `path`. A drive root keeps its separator (`C:\`), because
/// `C:` alone means the current folder of drive C.
fn parent(path: &str) -> &str {
    match path.rfind(['\\', '/']) {
        Some(i) if path[..i].ends_with(':') => &path[..=i],
        Some(i) => &path[..i],
        None => "",
    }
}

/// Deep nesting means a broken file; stop before the stack runs out.
const MAX_DEPTH: usize = 16;

fn take_map(b: &[u8], pos: &mut usize, depth: usize) -> Result<Vec<(Vec<u8>, Value)>, String> {
    if depth > MAX_DEPTH {
        return Err("shortcuts.vdf: maps nested too deep".into());
    }
    let mut out = Vec::new();
    loop {
        let kind = take_byte(b, pos)?;
        if kind == END {
            return Ok(out);
        }
        let key = take_cstr(b, pos)?.to_vec();
        let value = match kind {
            MAP => Value::Map(take_map(b, pos, depth + 1)?),
            STRING => Value::Str(take_cstr(b, pos)?.to_vec()),
            INT => {
                let raw = b
                    .get(*pos..*pos + 4)
                    .ok_or("shortcuts.vdf: file ends inside a number")?;
                *pos += 4;
                Value::Int(u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]))
            }
            other => {
                return Err(format!(
                    "shortcuts.vdf: unknown value type 0x{other:02X}; urakata does not change this file"
                ));
            }
        };
        out.push((key, value));
    }
}

fn take_byte(b: &[u8], pos: &mut usize) -> Result<u8, String> {
    let v = *b.get(*pos).ok_or("shortcuts.vdf: file ends too early")?;
    *pos += 1;
    Ok(v)
}

fn take_cstr<'a>(b: &'a [u8], pos: &mut usize) -> Result<&'a [u8], String> {
    let rest = &b[*pos..];
    let len = rest
        .iter()
        .position(|&c| c == 0)
        .ok_or("shortcuts.vdf: text without an end byte")?;
    *pos += len + 1;
    Ok(&rest[..len])
}

fn put(out: &mut Vec<u8>, key: &[u8], value: &Value) {
    let kind = match value {
        Value::Map(_) => MAP,
        Value::Str(_) => STRING,
        Value::Int(_) => INT,
    };
    out.push(kind);
    out.extend_from_slice(key);
    out.push(0);
    match value {
        Value::Map(fields) => {
            for (k, v) in fields {
                put(out, k, v);
            }
            out.push(END);
        }
        Value::Str(s) => {
            out.extend_from_slice(s);
            out.push(0);
        }
        Value::Int(v) => out.extend_from_slice(&v.to_le_bytes()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXE: &str = r"C:\Users\x\AppData\Local\Programs\urakata\urakata-app.exe";

    /// A file with one other game, built byte by byte as Valve's page
    /// describes the format.
    fn other_game() -> Vec<u8> {
        let mut b = vec![MAP];
        b.extend_from_slice(b"shortcuts\0");
        b.push(MAP);
        b.extend_from_slice(b"0\0");
        b.push(INT);
        b.extend_from_slice(b"appid\0");
        b.extend_from_slice(&0x8123_4567u32.to_le_bytes());
        b.push(STRING);
        b.extend_from_slice(b"AppName\0Other\0");
        b.push(STRING);
        b.extend_from_slice(b"Exe\0\"C:\\Games\\other.exe\"\0");
        b.push(MAP);
        b.extend_from_slice(b"tags\0");
        b.push(STRING);
        b.extend_from_slice(b"0\0favorite\0");
        b.push(END); // tags
        b.push(END); // entry 0
        b.push(END); // shortcuts
        b.push(END); // root
        b
    }

    #[test]
    fn round_trip_keeps_bytes() {
        let bytes = other_game();
        let list = parse(&bytes).unwrap();
        assert_eq!(list.entries.len(), 1);
        assert_eq!(serialize(&list), bytes);
    }

    #[test]
    fn empty_file_is_empty_list() {
        assert_eq!(parse(&[]).unwrap(), Shortcuts::default());
        let list = parse(&serialize(&Shortcuts::default())).unwrap();
        assert!(list.entries.is_empty());
    }

    #[test]
    fn refuses_unknown_types_and_broken_files() {
        let mut bytes = other_game();
        // Change the type byte of `appid` (after root header and "0" key).
        let at = 1 + b"shortcuts\0".len() + 1 + 2;
        assert_eq!(bytes[at], INT);
        bytes[at] = 0x07;
        assert!(parse(&bytes).unwrap_err().contains("0x07"));

        let full = other_game();
        for cut in 1..full.len() {
            assert!(parse(&full[..cut]).is_err(), "cut at {cut}");
        }
        let mut extra = full.clone();
        extra.push(0);
        assert!(parse(&extra).is_err());
        assert!(parse(b"\x00other\0\x08\x08").is_err());
    }

    #[test]
    fn add_then_add_again_then_move() {
        let mut list = parse(&other_game()).unwrap();
        assert_eq!(list.find(), None);
        assert_eq!(list.add(EXE), Added::New);
        assert_eq!(list.find(), Some(1));
        assert_eq!(list.add(EXE), Added::Unchanged);

        let entry = &list.entries[1];
        assert_eq!(
            field(entry, b"Exe"),
            Some(&Value::Str(format!("\"{EXE}\"").into_bytes()))
        );
        assert_eq!(
            field(entry, b"StartDir"),
            Some(&Value::Str(
                br#""C:\Users\x\AppData\Local\Programs\urakata""#.to_vec()
            ))
        );
        assert_eq!(
            field(entry, b"AppName"),
            Some(&Value::Str(b"urakata".to_vec()))
        );
        assert_eq!(
            field(entry, b"LaunchOptions"),
            Some(&Value::Str(b"--big-picture".to_vec()))
        );

        // The user renamed the entry in Steam; then urakata moved.
        let Value::Map(fields) = &mut list.entries[1] else {
            panic!()
        };
        set_str(fields, b"AppName", b"Displays");
        let moved = Changes {
            path: true,
            ..Changes::default()
        };
        assert_eq!(
            list.add(r"D:\urakata\URAKATA-APP.EXE"),
            Added::Updated(moved)
        );
        assert_eq!(list.entries.len(), 2);
        assert_eq!(
            field(&list.entries[1], b"AppName"),
            Some(&Value::Str(b"Displays".to_vec()))
        );
        assert_eq!(
            field(&list.entries[1], b"StartDir"),
            Some(&Value::Str(br#""D:\urakata""#.to_vec()))
        );

        // An entry from an older urakata has no options: add gives them,
        // but keeps options the user typed.
        let Value::Map(fields) = &mut list.entries[1] else {
            panic!()
        };
        set_str(fields, b"LaunchOptions", b"");
        let options = Changes {
            options: true,
            ..Changes::default()
        };
        assert_eq!(
            list.add(r"D:\urakata\URAKATA-APP.EXE"),
            Added::Updated(options)
        );
        assert_eq!(options.text(), "entry updated: launch options");
        assert_eq!(
            Changes {
                path: true,
                app_id: true,
                ..options
            }
            .text(),
            "entry updated: new program path, launch options, appid"
        );
        assert_eq!(
            field(&list.entries[1], b"LaunchOptions"),
            Some(&Value::Str(b"--big-picture".to_vec()))
        );
        let Value::Map(fields) = &mut list.entries[1] else {
            panic!()
        };
        set_str(fields, b"LaunchOptions", b"--windowed");
        assert_eq!(list.add(r"D:\urakata\URAKATA-APP.EXE"), Added::Unchanged);

        // The written file reads back the same.
        let again = parse(&serialize(&list)).unwrap();
        assert_eq!(again, list);
    }

    #[test]
    fn remove_keeps_other_games_and_renumbers() {
        let mut list = Shortcuts::default();
        list.add(EXE);
        list.entries.extend(parse(&other_game()).unwrap().entries);
        assert_eq!(list.remove(), 1);
        assert_eq!(list.remove(), 0);
        // The other game is now entry "0" again: same bytes as the input.
        assert_eq!(serialize(&list), other_game());
    }

    #[test]
    fn add_updates_every_duplicate_and_keeps_drive_roots() {
        let mut list = Shortcuts::default();
        list.add(EXE);
        list.add(EXE);
        assert_eq!(list.entries.len(), 1);
        list.entries.push(list.entries[0].clone());
        assert!(matches!(
            list.add(r"C:\urakata-app.exe"),
            Added::Updated(Changes { path: true, .. })
        ));
        for entry in &list.entries {
            assert_eq!(
                field(entry, b"StartDir"),
                Some(&Value::Str(br#""C:\""#.to_vec()))
            );
        }
        assert_eq!(parent(r"D:\a\b.exe"), r"D:\a");
        assert_eq!(parent("b.exe"), "");
    }

    #[test]
    fn only_the_file_name_makes_an_entry_ours() {
        let mut list = Shortcuts::default();
        list.entries.push(new_entry(
            1,
            "x",
            r#""C:\urakata-app.exe.bak\other.exe""#,
            "",
        ));
        list.entries
            .push(new_entry(2, "x", r#""C:\x\not-urakata-app.exe""#, ""));
        list.entries.push(new_entry(3, "x", "urakata-app.exe", ""));
        assert_eq!(list.find(), Some(2));
    }

    #[test]
    fn app_id_is_crc32_with_top_bit_and_unique() {
        // CRC-32 check value from the standard ("123456789").
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        let id = app_id(APP_NAME, &format!("\"{EXE}\""));
        assert_ne!(id & 0x8000_0000, 0);

        let mut list = Shortcuts::default();
        list.entries.push(new_entry(id, "x", "\"other.exe\"", ""));
        list.add(EXE);
        let Some(Value::Int(new)) = field(&list.entries[1], b"appid") else {
            panic!()
        };
        assert_ne!(*new, id);
        assert_ne!(*new & 0x8000_0000, 0);
    }

    #[test]
    fn icons_and_app_ids_of_our_entries() {
        let grid = r"C:\Steam\userdata\1\config\grid";
        let path = |id: u32| format!(r"{grid}\{id}_icon.ico");
        let icon = |list: &Shortcuts, i: usize| match field(&list.entries[i], b"icon") {
            Some(Value::Str(v)) => String::from_utf8_lossy(v).into_owned(),
            _ => panic!("no icon field"),
        };
        let mut list = parse(&other_game()).unwrap();
        assert!(list.app_ids().is_empty());
        list.add(EXE);
        let ids = list.app_ids();
        assert_eq!(ids.len(), 1);

        // A new entry gets our icon; the other game is not touched.
        assert!(list.set_icons(path));
        assert_eq!(icon(&list, 1), path(ids[0]));
        assert_eq!(field(&list.entries[0], b"icon"), None);
        assert!(!list.set_icons(path));

        // Steam moved: our old icon path follows.
        assert!(list.set_icons(|id| format!(r"D:\Steam\grid\{id}_icon.ico")));
        // The user chose another icon in Steam: it stays.
        let Value::Map(fields) = &mut list.entries[1] else {
            panic!()
        };
        set_str(fields, b"icon", br"C:\Pictures\mine.png");
        assert!(!list.set_icons(path));
        assert_eq!(icon(&list, 1), r"C:\Pictures\mine.png");

        // A duplicate with the same appid counts once.
        list.entries.push(list.entries[1].clone());
        assert_eq!(list.app_ids(), ids);
    }

    #[test]
    fn add_gives_an_appid_to_an_entry_without_one() {
        let mut list = Shortcuts::default();
        list.add(EXE);
        let Value::Map(fields) = &mut list.entries[0] else {
            panic!()
        };
        fields.retain(|(k, _)| k != b"appid");
        assert!(list.app_ids().is_empty());
        let id = Changes {
            app_id: true,
            ..Changes::default()
        };
        assert_eq!(list.add(EXE), Added::Updated(id));
        assert_eq!(list.app_ids(), [app_id(APP_NAME, &format!("\"{EXE}\""))]);
        assert_eq!(list.add(EXE), Added::Unchanged);
    }
}
