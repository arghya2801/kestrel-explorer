//! Places for the sidebar: known user folders, drives, and the sub-folders a
//! tree node shows when expanded.

use std::io;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use windows::Win32::Storage::FileSystem::{GetDriveTypeW, GetLogicalDrives, GetVolumeInformationW};
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::System::WindowsProgramming::{
    DRIVE_CDROM, DRIVE_FIXED, DRIVE_RAMDISK, DRIVE_REMOTE, DRIVE_REMOVABLE,
};
use windows::Win32::UI::Shell::{
    FOLDERID_Desktop, FOLDERID_Documents, FOLDERID_Downloads, FOLDERID_Music, FOLDERID_Pictures,
    FOLDERID_Videos, KNOWN_FOLDER_FLAG, SHGetKnownFolderPath,
};
use windows::core::{GUID, PCWSTR};

use crate::listing::list_dir;
use crate::sort::{Sort, SortKey, sorted_order};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DriveKind {
    Fixed,
    Removable,
    Optical,
    Network,
    Ram,
    Other,
}

#[derive(Clone, Debug)]
pub struct Drive {
    pub letter: char,
    pub root: PathBuf,
    pub kind: DriveKind,
}

pub fn drive_letters(mask: u32) -> Vec<char> {
    (0..26u8).filter(|i| mask & (1 << i) != 0).map(|i| (b'A' + i) as char).collect()
}

/// Drive letters and types. Doesn't touch the media, so it never blocks.
pub fn drives() -> Vec<Drive> {
    drive_letters(unsafe { GetLogicalDrives() })
        .into_iter()
        .map(|letter| {
            let root = format!("{letter}:\\");
            let wide: Vec<u16> = root.encode_utf16().chain([0]).collect();
            let kind = match unsafe { GetDriveTypeW(PCWSTR(wide.as_ptr())) } {
                DRIVE_FIXED => DriveKind::Fixed,
                DRIVE_REMOVABLE => DriveKind::Removable,
                DRIVE_CDROM => DriveKind::Optical,
                DRIVE_REMOTE => DriveKind::Network,
                DRIVE_RAMDISK => DriveKind::Ram,
                _ => DriveKind::Other,
            };
            Drive { letter, root: root.into(), kind }
        })
        .collect()
}

/// The volume label. Reads the media: worker threads only.
pub fn volume_label(root: &Path) -> Option<String> {
    let wide: Vec<u16> = root.as_os_str().encode_wide().chain([0]).collect();
    let mut buf = [0u16; 261];
    unsafe { GetVolumeInformationW(PCWSTR(wide.as_ptr()), Some(&mut buf), None, None, None, None) }.ok()?;
    let len = buf.iter().position(|&c| c == 0)?;
    Some(String::from_utf16_lossy(&buf[..len]))
}

/// "Windows (C:)", or a name for the kind of drive when it has no label.
pub fn drive_name(letter: char, kind: DriveKind, label: Option<&str>) -> String {
    let name = match (label, kind) {
        (Some(l), _) if !l.is_empty() => l,
        (_, DriveKind::Fixed) => "Local Disk",
        (_, DriveKind::Removable) => "USB Drive",
        (_, DriveKind::Optical) => "CD Drive",
        (_, DriveKind::Network) => "Network Drive",
        (_, DriveKind::Ram) => "RAM Disk",
        (_, DriveKind::Other) => "Drive",
    };
    format!("{name} ({letter}:)")
}

/// The user's standard folders. Reads the registry only.
pub fn known_folders() -> Vec<(&'static str, PathBuf)> {
    let ids: [(&str, GUID); 6] = [
        ("Desktop", FOLDERID_Desktop),
        ("Downloads", FOLDERID_Downloads),
        ("Documents", FOLDERID_Documents),
        ("Pictures", FOLDERID_Pictures),
        ("Music", FOLDERID_Music),
        ("Videos", FOLDERID_Videos),
    ];
    ids.into_iter()
        .filter_map(|(name, id)| unsafe {
            let p = SHGetKnownFolderPath(&id, KNOWN_FOLDER_FLAG(0), None).ok()?;
            let path = PathBuf::from(String::from_utf16_lossy(p.as_wide()));
            CoTaskMemFree(Some(p.0 as *const _));
            Some((name, path))
        })
        .collect()
}

/// Names of the folders directly inside `dir`, in display order.
pub fn subfolders(dir: &Path, show_hidden: bool) -> io::Result<Vec<String>> {
    let s = list_dir(dir)?;
    let order = sorted_order(&s, Sort { key: SortKey::Name, descending: false }, show_hidden);
    Ok(order.into_iter().map(|i| i as usize).filter(|&i| s.is_dir(i)).map(|i| s.name_string(i)).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn drive_mask_bits_map_to_letters() {
        assert_eq!(drive_letters(0b101), ['A', 'C']);
        assert_eq!(drive_letters(1 << 25), ['Z']);
        assert!(drive_letters(0).is_empty());
    }

    #[test]
    fn drive_names_use_label_or_kind() {
        assert_eq!(drive_name('C', DriveKind::Fixed, Some("Windows")), "Windows (C:)");
        assert_eq!(drive_name('C', DriveKind::Fixed, Some("")), "Local Disk (C:)");
        assert_eq!(drive_name('E', DriveKind::Removable, None), "USB Drive (E:)");
        assert_eq!(drive_name('Z', DriveKind::Network, None), "Network Drive (Z:)");
        assert_eq!(drive_name('D', DriveKind::Optical, None), "CD Drive (D:)");
    }

    #[test]
    fn this_machine_has_a_system_drive() {
        let drives = drives();
        let sys = std::env::var("SystemDrive").unwrap(); // "C:"
        let c =
            drives.iter().find(|d| d.root.to_string_lossy().starts_with(&sys)).expect("system drive listed");
        assert_eq!(c.kind, DriveKind::Fixed);
    }

    #[test]
    fn known_folders_are_absolute_and_exist() {
        let k = known_folders();
        assert_eq!(k.len(), 6);
        assert_eq!(k[0].0, "Desktop");
        assert!(k.iter().all(|(_, p)| p.is_absolute()));
        assert!(k[0].1.is_dir());
    }

    #[test]
    fn subfolders_are_dirs_only_sorted_and_respect_hidden() {
        let d = std::env::temp_dir().join(format!("fep-places-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        for name in ["b10", "b9", "A", ".hid"] {
            fs::create_dir_all(d.join(name)).unwrap();
        }
        fs::write(d.join("file.txt"), "").unwrap();
        let hid: Vec<u16> = d.join(".hid").as_os_str().encode_wide().chain([0]).collect();
        unsafe {
            windows::Win32::Storage::FileSystem::SetFileAttributesW(
                windows::core::PCWSTR(hid.as_ptr()),
                windows::Win32::Storage::FileSystem::FILE_ATTRIBUTE_HIDDEN,
            )
        }
        .unwrap();

        assert_eq!(subfolders(&d, false).unwrap(), ["A", "b9", "b10"]);
        assert_eq!(subfolders(&d, true).unwrap(), [".hid", "A", "b9", "b10"]);
        assert!(subfolders(&d.join("missing"), false).is_err());
        fs::remove_dir_all(d).unwrap();
    }
}
