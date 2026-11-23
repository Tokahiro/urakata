//! The library artwork of the Steam entry: the icon and the four grid
//! images in `userdata\<id>\config\grid\`, named by the entry's `appid`.
//!
//! File names: `<appid>p.png` capsule
//! (600x900), `<appid>.png` wide capsule (920x430), `<appid>_hero.png`
//! (1920x620), `<appid>_logo.png` (transparent). The icon is not read from
//! the grid folder by name; the entry's `icon` field points to
//! `<appid>_icon.ico`, which urakata puts in the same folder.
//!
//! The images are part of the program (`include_bytes!`), so `steam-entry
//! --add` works from any copy of `urakata.exe`. `<appid>_urakata.txt`
//! lists the files urakata wrote, so a newer version replaces its own old
//! images but never the user's. No Windows calls here, so the rules have
//! unit tests.

use std::path::{Path, PathBuf};

/// Name part after the appid, file type, and urakata's content.
const IMAGES: [(&str, &str, &[u8]); 5] = [
    (
        "p",
        "png",
        include_bytes!("../../../assets/steam/capsule.png"),
    ),
    ("", "png", include_bytes!("../../../assets/steam/wide.png")),
    (
        "_hero",
        "png",
        include_bytes!("../../../assets/steam/hero.png"),
    ),
    (
        "_logo",
        "png",
        include_bytes!("../../../assets/steam/logo.png"),
    ),
    (
        "_icon",
        "ico",
        include_bytes!("../../../assets/logo/urakata.ico"),
    ),
];

/// Steam keeps the file type of the picture a user chooses, so the user's
/// own artwork can be a JPEG file under the same name.
const IMAGE_TYPES: [&str; 3] = ["png", "jpg", "jpeg"];

/// How many files make the full set.
pub const COUNT: usize = IMAGES.len();

/// The icon file of the entry with this `appid`, for its `icon` field.
pub fn icon_path(grid: &Path, app_id: u32) -> PathBuf {
    grid.join(format!("{app_id}_icon.ico"))
}

/// Whether an `icon` field value is the icon urakata sets for this `appid`
/// in any grid folder (the file name decides, as Steam's folder can move).
pub fn is_our_icon(value: &str, app_id: u32) -> bool {
    let value = value.trim().trim_matches('"');
    let name = value.rsplit(['\\', '/']).next().unwrap_or(value);
    name.eq_ignore_ascii_case(&format!("{app_id}_icon.ico"))
}

/// What `install` found and did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Installed {
    /// Files written now (missing, or urakata's from an older version).
    pub written: usize,
    /// The user's own artwork, set in Steam under the same name. It stays.
    pub kept: usize,
}

/// The names one image can have in the grid folder: urakata's first.
fn variants(app_id: u32, part: &str, kind: &str) -> Vec<String> {
    let mut names = vec![format!("{app_id}{part}.{kind}")];
    if kind != "ico" {
        names.extend(
            IMAGE_TYPES
                .iter()
                .filter(|t| **t != kind)
                .map(|t| format!("{app_id}{part}.{t}")),
        );
    }
    names
}

/// The list of files urakata wrote, with a CRC-32 of each, so a later
/// version can tell its own old images from the user's artwork.
fn record_path(grid: &Path, app_id: u32) -> PathBuf {
    grid.join(format!("{app_id}_urakata.txt"))
}

fn read_record(grid: &Path, app_id: u32) -> Vec<(String, u32)> {
    std::fs::read_to_string(record_path(grid, app_id))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| {
            let (name, crc) = l.split_once(' ')?;
            Some((name.to_string(), u32::from_str_radix(crc, 16).ok()?))
        })
        .collect()
}

/// Puts urakata's images into `grid` (made if needed) where no image of
/// the user's own is. A file that urakata wrote before is replaced when
/// the image changed.
pub fn install(grid: &Path, app_id: u32) -> Result<Installed, String> {
    let mut done = Installed::default();
    let mut record = read_record(grid, app_id);
    let written_by_us = |record: &[(String, u32)], name: &str, data: &[u8]| {
        record
            .iter()
            .any(|(n, crc)| n == name && *crc == crate::shortcuts::crc32(data))
    };
    for (part, kind, data) in IMAGES {
        let names = variants(app_id, part, kind);
        let mut ours_ok = false;
        let mut users = false;
        for name in &names {
            match std::fs::read(grid.join(name)) {
                Ok(old) if name == &names[0] && old == data => ours_ok = true,
                Ok(old) if written_by_us(&record, name, &old) => {}
                Ok(_) => users = true,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(format!("cannot read {name}: {e}")),
            }
        }
        if users {
            done.kept += 1;
            continue;
        }
        if ours_ok {
            continue;
        }
        std::fs::create_dir_all(grid)
            .map_err(|e| format!("cannot make Steam's grid folder: {e}"))?;
        write_new(&grid.join(&names[0]), data)?;
        record.retain(|(n, _)| n != &names[0]);
        record.push((names[0].clone(), crate::shortcuts::crc32(data)));
        done.written += 1;
    }
    if done.written > 0 {
        let text: String = record
            .iter()
            .map(|(n, crc)| format!("{n} {crc:08x}\n"))
            .collect();
        write_new(&record_path(grid, app_id), text.as_bytes())?;
    }
    Ok(done)
}

/// How many images of the set are in `grid`, urakata's or the user's.
pub fn present(grid: &Path, app_id: u32) -> usize {
    IMAGES
        .iter()
        .filter(|(part, kind, _)| {
            variants(app_id, part, kind)
                .iter()
                .any(|n| grid.join(n).is_file())
        })
        .count()
}

/// Deletes the images of the entry (urakata's and the user's own: the
/// entry is gone, so they show nothing) and urakata's list of them. Other
/// files stay. Returns how many images were deleted.
pub fn remove(grid: &Path, app_id: u32) -> Result<usize, String> {
    let mut n = 0;
    let names = IMAGES
        .iter()
        .flat_map(|(part, kind, _)| variants(app_id, part, kind));
    for name in names {
        if delete(&grid.join(&name))? {
            n += 1;
        }
    }
    delete(&record_path(grid, app_id))?;
    Ok(n)
}

fn delete(file: &Path) -> Result<bool, String> {
    match std::fs::remove_file(file) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(format!("cannot delete {}: {e}", file.display())),
    }
}

/// Writes to a temporary name, then renames, so Steam never reads half a
/// file.
fn write_new(file: &Path, data: &[u8]) -> Result<(), String> {
    let mut temp = file.as_os_str().to_owned();
    temp.push(".urakata-new");
    let temp = PathBuf::from(temp);
    std::fs::write(&temp, data)
        .and_then(|()| std::fs::rename(&temp, file))
        .map_err(|e| {
            let _ = std::fs::remove_file(&temp);
            format!("cannot write {}: {e}", file.display())
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: u32 = 0x8123_4567;

    fn temp_grid(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("urakata-art-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir.join("grid")
    }

    #[test]
    fn names_follow_the_appid() {
        let names: Vec<String> = IMAGES
            .iter()
            .map(|(p, k, _)| variants(ID, p, k)[0].clone())
            .collect();
        assert_eq!(
            names,
            [
                "2166572391p.png",
                "2166572391.png",
                "2166572391_hero.png",
                "2166572391_logo.png",
                "2166572391_icon.ico",
            ]
        );
        assert!(is_our_icon(
            r"C:\Steam\userdata\1\config\grid\2166572391_icon.ico",
            ID
        ));
        assert!(!is_our_icon(
            r"C:\Steam\userdata\1\config\grid\1_icon.ico",
            ID
        ));
        assert!(!is_our_icon(r"C:\Games\other.ico", ID));
        assert!(!is_our_icon("", ID));
    }

    #[test]
    fn images_have_the_sizes_steam_expects() {
        // PNG: width and height are big-endian at bytes 16..24.
        let size = |d: &[u8]| {
            assert_eq!(&d[..8], b"\x89PNG\r\n\x1a\n");
            (
                u32::from_be_bytes(d[16..20].try_into().unwrap()),
                u32::from_be_bytes(d[20..24].try_into().unwrap()),
            )
        };
        assert_eq!(size(IMAGES[0].2), (600, 900));
        assert_eq!(size(IMAGES[1].2), (920, 430));
        assert_eq!(size(IMAGES[2].2), (1920, 620));
        let (w, h) = size(IMAGES[3].2);
        assert!(w <= 1280 && h <= 720, "logo {w}x{h}");
        // ICO header: reserved 0, type 1.
        assert_eq!(&IMAGES[4].2[..4], b"\0\0\x01\0");
    }

    #[test]
    fn install_then_again_then_remove() {
        let grid = temp_grid("cycle");
        let other = grid.join("12345p.png");
        assert_eq!(present(&grid, ID), 0);
        assert_eq!(
            install(&grid, ID).unwrap(),
            Installed {
                written: COUNT,
                kept: 0
            }
        );
        std::fs::write(&other, b"other game").unwrap();
        assert_eq!(present(&grid, ID), COUNT);
        assert_eq!(install(&grid, ID).unwrap(), Installed::default());
        assert!(icon_path(&grid, ID).is_file());

        assert_eq!(remove(&grid, ID).unwrap(), COUNT);
        assert_eq!(remove(&grid, ID).unwrap(), 0);
        assert_eq!(present(&grid, ID), 0);
        // Other games' artwork stays, and no temporary file is left.
        let left: Vec<_> = std::fs::read_dir(&grid)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(left, ["12345p.png"]);
        let _ = std::fs::remove_dir_all(grid.parent().unwrap());
    }

    #[test]
    fn install_keeps_the_users_own_artwork() {
        let grid = temp_grid("own");
        std::fs::create_dir_all(&grid).unwrap();
        let hero = grid.join(format!("{ID}_hero.png"));
        std::fs::write(&hero, b"own hero").unwrap();
        assert_eq!(
            install(&grid, ID).unwrap(),
            Installed {
                written: COUNT - 1,
                kept: 1
            }
        );
        assert_eq!(std::fs::read(&hero).unwrap(), b"own hero");
        assert_eq!(
            install(&grid, ID).unwrap(),
            Installed {
                written: 0,
                kept: 1
            }
        );
        let _ = std::fs::remove_dir_all(grid.parent().unwrap());
    }

    #[test]
    fn a_users_jpeg_counts_and_goes_with_remove() {
        let grid = temp_grid("jpeg");
        std::fs::create_dir_all(&grid).unwrap();
        let logo = grid.join(format!("{ID}_logo.jpg"));
        std::fs::write(&logo, b"own logo").unwrap();
        assert_eq!(present(&grid, ID), 1);
        assert_eq!(
            install(&grid, ID).unwrap(),
            Installed {
                written: COUNT - 1,
                kept: 1
            }
        );
        assert!(!grid.join(format!("{ID}_logo.png")).exists());
        assert_eq!(present(&grid, ID), COUNT);
        assert_eq!(remove(&grid, ID).unwrap(), COUNT);
        assert_eq!(std::fs::read_dir(&grid).unwrap().count(), 0);
        let _ = std::fs::remove_dir_all(grid.parent().unwrap());
    }

    #[test]
    fn an_old_urakata_image_is_replaced() {
        let grid = temp_grid("old");
        install(&grid, ID).unwrap();
        // As if an older version wrote another hero.
        let hero = grid.join(format!("{ID}_hero.png"));
        std::fs::write(&hero, b"old hero").unwrap();
        let record = record_path(&grid, ID);
        let text = std::fs::read_to_string(&record).unwrap().replace(
            &format!("{:08x}", crate::shortcuts::crc32(IMAGES[2].2)),
            &format!("{:08x}", crate::shortcuts::crc32(b"old hero")),
        );
        std::fs::write(&record, text).unwrap();
        assert_eq!(
            install(&grid, ID).unwrap(),
            Installed {
                written: 1,
                kept: 0
            }
        );
        assert_eq!(std::fs::read(&hero).unwrap(), IMAGES[2].2);
        // The new content is recorded: a later --add has nothing to do.
        assert_eq!(install(&grid, ID).unwrap(), Installed::default());
        let _ = std::fs::remove_dir_all(grid.parent().unwrap());
    }
}
