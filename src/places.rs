//! Places for the sidebar: known user folders, drives, and the sub-folders a
//! tree node shows when expanded. Also This PC, the page listing the drives.

use std::io;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use windows::Win32::Storage::FileSystem::{
    FILE_ATTRIBUTE_DIRECTORY, GetDiskFreeSpaceExW, GetDriveTypeW, GetLogicalDrives, GetVolumeInformationW,
};
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::System::WindowsProgramming::{
    DRIVE_CDROM, DRIVE_FIXED, DRIVE_RAMDISK, DRIVE_REMOTE, DRIVE_REMOVABLE,
};
use windows::Win32::UI::Shell::{
    FOLDERID_Desktop, FOLDERID_Documents, FOLDERID_Downloads, FOLDERID_Music, FOLDERID_Pictures,
    FOLDERID_Videos, KNOWN_FOLDER_FLAG, SHGetKnownFolderPath,
};
use windows::core::{GUID, PCWSTR};

use crate::browser::same_dir;
use crate::listing::{Snapshot, list_dir};
use crate::sort::{Sort, SortKey, sorted_order};

/// The page listing the drives, like Explorer's. A name rather than a folder: it shows as is
/// in the address bar and on its tab, and `list_this_pc` lists it.
pub const THIS_PC: &str = "This PC";

pub fn this_pc() -> PathBuf {
    PathBuf::from(THIS_PC)
}

pub fn is_this_pc(path: &Path) -> bool {
    path.as_os_str().eq_ignore_ascii_case(THIS_PC)
}

/// A drive on the This PC page, beside its folder entry in the snapshot.
#[derive(Clone, Debug)]
pub struct DriveRow {
    pub root: PathBuf,
    /// The Type column: "Local Disk, 120 GB free".
    pub detail: String,
    /// Index in the system image list.
    pub icon: i32,
}

/// This PC: a folder entry per drive, named as in the sidebar, with its capacity as the size.
/// Reads every drive but network ones, which can take long to answer: worker threads only.
pub fn list_this_pc() -> io::Result<Snapshot> {
    let mut snap = Snapshot::new(this_pc());
    for d in drives() {
        let network = d.kind == DriveKind::Network;
        let label = if network { None } else { volume_label(&d.root) };
        let (mut free, mut total) = (0u64, 0u64);
        let wide = windows::core::HSTRING::from(d.root.as_os_str());
        let sized = !network
            && unsafe { GetDiskFreeSpaceExW(&wide, Some(&mut free), Some(&mut total), None) }.is_ok();
        let name: Vec<u16> = drive_name(d.letter, d.kind, label.as_deref()).encode_utf16().collect();
        snap.add(&name, FILE_ATTRIBUTE_DIRECTORY.0, if sized { total } else { 0 }, 0);
        let detail = match sized {
            true => format!("{}, {} free", kind_name(d.kind), crate::format::bytes(free)),
            false => kind_name(d.kind).to_string(),
        };
        let icon = crate::icons::file_icon(&d.root).unwrap_or(0);
        snap.drives.push(DriveRow { root: d.root, detail, icon });
    }
    Ok(snap)
}

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

fn kind_name(kind: DriveKind) -> &'static str {
    match kind {
        DriveKind::Fixed => "Local Disk",
        DriveKind::Removable => "USB Drive",
        DriveKind::Optical => "CD Drive",
        DriveKind::Network => "Network Drive",
        DriveKind::Ram => "RAM Disk",
        DriveKind::Other => "Drive",
    }
}

/// "Windows (C:)", or a name for the kind of drive when it has no label.
pub fn drive_name(letter: char, kind: DriveKind, label: Option<&str>) -> String {
    let name = label.filter(|l| !l.is_empty()).unwrap_or(kind_name(kind));
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

/// Quick access: the known folders the user hasn't removed, then the pinned folders.
pub fn quick_access(known: &[PathBuf], pins: &[PathBuf], unpinned: &[PathBuf]) -> Vec<PathBuf> {
    let mut shown: Vec<PathBuf> =
        known.iter().filter(|k| !unpinned.iter().any(|u| same_dir(u, k))).cloned().collect();
    for p in pins {
        if !shown.iter().any(|s| same_dir(s, p)) {
            shown.push(p.clone());
        }
    }
    shown
}

/// Adds `dir` to Quick access, or takes it out. A known folder taken out is remembered
/// in `unpinned`; any other folder is a pin.
pub fn set_quick_access(
    dir: &Path,
    on: bool,
    known: &[PathBuf],
    pins: &mut Vec<PathBuf>,
    unpinned: &mut Vec<PathBuf>,
) {
    pins.retain(|p| !same_dir(p, dir));
    unpinned.retain(|p| !same_dir(p, dir));
    if known.iter().any(|k| same_dir(k, dir)) {
        if !on {
            unpinned.push(dir.to_path_buf());
        }
    } else if on {
        pins.push(dir.to_path_buf());
    }
}

/// Labels for `paths`: each folder's name, followed by as many of its parents as it takes to
/// tell it from a different folder of the same name ("MFA221 · bin", "MFA221 · target").
pub fn short_names(paths: &[PathBuf]) -> Vec<String> {
    // Name first, then each parent's name, ending with the root ("C:\").
    let parts: Vec<Vec<String>> = paths
        .iter()
        .map(|p| {
            p.ancestors()
                .map(|a| {
                    a.file_name().map_or_else(|| a.display().to_string(), |n| n.to_string_lossy().into())
                })
                .filter(|n| !n.is_empty())
                .collect()
        })
        .collect();
    let same = |a: &[String], b: &[String]| {
        a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.eq_ignore_ascii_case(y))
    };
    paths
        .iter()
        .zip(&parts)
        .map(|(path, mine)| {
            let Some(name) = mine.first() else { return path.display().to_string() };
            let depth = (0..mine.len())
                .find(|&d| {
                    !paths.iter().zip(&parts).any(|(other, theirs)| {
                        !same_dir(other, path) && theirs.len() > d && same(&theirs[..=d], &mine[..=d])
                    })
                })
                .unwrap_or(mine.len() - 1);
            if depth == 0 {
                return name.clone();
            }
            let parents: Vec<&str> = mine[1..=depth].iter().rev().map(|p| p.trim_end_matches('\\')).collect();
            format!("{name} · {}", parents.join("\\"))
        })
        .collect()
}

/// Names of the folders directly inside `dir`, in display order.
pub fn subfolders(dir: &Path, show_hidden: bool) -> io::Result<Vec<String>> {
    let s = list_dir(dir)?;
    let order =
        sorted_order(&s, Sort { key: SortKey::Name, descending: false }, show_hidden, &Default::default());
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
    fn known_folders_can_leave_quick_access_and_come_back() {
        let known = [PathBuf::from(r"C:\U\Desktop"), PathBuf::from(r"C:\U\Music")];
        let (mut pins, mut unpinned) = (Vec::new(), Vec::new());
        set_quick_access(Path::new(r"c:\u\music"), false, &known, &mut pins, &mut unpinned);
        set_quick_access(Path::new(r"D:\work"), true, &known, &mut pins, &mut unpinned);
        assert_eq!(
            quick_access(&known, &pins, &unpinned),
            [PathBuf::from(r"C:\U\Desktop"), "D:\\work".into()]
        );
        assert!(pins == [PathBuf::from(r"D:\work")] && unpinned.len() == 1);

        set_quick_access(Path::new(r"C:\U\Music"), true, &known, &mut pins, &mut unpinned);
        set_quick_access(Path::new(r"d:\WORK"), false, &known, &mut pins, &mut unpinned);
        assert_eq!(quick_access(&known, &pins, &unpinned), known);
        assert!(pins.is_empty() && unpinned.is_empty());
        // A pin that is also a known folder shows once.
        assert_eq!(quick_access(&known, &[PathBuf::from(r"c:\u\desktop")], &[]), known);
    }

    #[test]
    fn short_names_add_parents_only_to_tell_folders_apart() {
        let paths: Vec<PathBuf> = [
            r"C:\mkvd\bin\MFA221",
            r"C:\Code\mfa\target\MFA221",
            r"C:\a\x\lib",
            r"C:\b\x\lib",
            r"D:\bak",
            r"D:\bak",
            r"C:\",
            THIS_PC,
        ]
        .map(PathBuf::from)
        .into();
        assert_eq!(
            short_names(&paths),
            ["MFA221 · bin", "MFA221 · target", r"lib · a\x", r"lib · b\x", "bak", "bak", r"C:\", "This PC"]
        );
        assert_eq!(short_names(&[r"C:\x".into(), r"D:\x".into()]), [r"x · C:", r"x · D:"]);
    }

    #[test]
    fn this_pc_lists_the_drives_with_their_roots() {
        assert!(is_this_pc(Path::new("this pc")) && !is_this_pc(Path::new(r"C:\This PC")));
        let pc = list_this_pc().unwrap();
        assert_eq!(pc.len(), drives().len());
        assert_eq!(pc.drives.len(), pc.len());
        let sys = std::env::var("SystemDrive").unwrap() + "\\";
        let c = pc.drives.iter().position(|d| d.root == Path::new(&sys)).expect("system drive listed");
        assert!(pc.is_dir(c) && pc.entries[c].size > 0 && pc.drives[c].detail.contains("free"));
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
