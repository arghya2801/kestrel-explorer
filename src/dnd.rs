//! Drag and drop decisions, Explorer-style: within a drive a plain drag moves,
//! across drives it copies; Ctrl copies, Shift moves.

use std::path::{Component, Path, PathBuf, Prefix};

use windows::Win32::System::Com::{DVASPECT_CONTENT, FORMATETC, IDataObject, TYMED_HGLOBAL};
use windows::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};
use windows::Win32::System::Ole::{CF_HDROP, ReleaseStgMedium};
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::{
    BHID_DataObject, ILFree, SHCreateShellItemArrayFromIDLists, SHParseDisplayName,
};
use windows::core::HSTRING;

pub const NONE: u32 = 0;
pub const COPY: u32 = 1;
pub const MOVE: u32 = 2;
pub const LINK: u32 = 4;
pub const MK_SHIFT: u32 = 0x4;
pub const MK_CONTROL: u32 = 0x8;
pub const MK_ALT: u32 = 0x20;

/// "c:" or `\\server\share`, lowercase.
fn volume(path: &Path) -> Option<String> {
    let Some(Component::Prefix(p)) = path.components().next() else { return None };
    match p.kind() {
        Prefix::Disk(d) | Prefix::VerbatimDisk(d) => Some(format!("{}:", (d as char).to_ascii_lowercase())),
        Prefix::UNC(server, share) | Prefix::VerbatimUNC(server, share) => {
            Some(format!(r"\\{}\{}", server.to_string_lossy(), share.to_string_lossy()).to_lowercase())
        }
        _ => None,
    }
}

pub fn same_volume(a: &Path, b: &Path) -> bool {
    let va = volume(a);
    va.is_some() && va == volume(b)
}

/// Paths compare without case on Windows.
fn lower(p: &Path) -> String {
    p.to_string_lossy().trim_end_matches('\\').to_lowercase()
}

/// What dropping `sources` into `target` should do, given the modifier keys
/// held and the effects the drag source allows. Links (Alt, Ctrl+Shift) aren't
/// supported; neither is dropping into the items' own folder or into itself.
pub fn drop_effect(keys: u32, allowed: u32, sources: &[PathBuf], target: &Path) -> u32 {
    let link = keys & MK_ALT != 0 || keys & (MK_CONTROL | MK_SHIFT) == MK_CONTROL | MK_SHIFT;
    if sources.is_empty() || link {
        return NONE;
    }
    let t = lower(target);
    let pointless = sources.iter().any(|s| {
        let s = lower(s);
        s.rsplit_once('\\').is_some_and(|(parent, _)| parent == t)
            || t == s
            || t.starts_with(&format!("{s}\\"))
    });
    if pointless {
        return NONE;
    }
    let wanted = if keys & MK_CONTROL != 0 {
        COPY
    } else if keys & MK_SHIFT != 0 || same_volume(&sources[0], target) {
        MOVE
    } else {
        COPY
    };
    [wanted, COPY, MOVE].into_iter().find(|e| allowed & e != 0).unwrap_or(NONE)
}

/// The shell's own data object for `paths`: what Explorer hands out when
/// dragging the same files, so every drop target understands it.
pub fn data_object(paths: &[PathBuf]) -> windows::core::Result<IDataObject> {
    let mut pidls: Vec<*mut ITEMIDLIST> = Vec::with_capacity(paths.len());
    let result = (|| unsafe {
        for p in paths {
            let mut pidl = std::ptr::null_mut();
            SHParseDisplayName(&HSTRING::from(p.as_os_str()), None, &mut pidl, 0, None)?;
            pidls.push(pidl);
        }
        let ids: Vec<*const ITEMIDLIST> = pidls.iter().map(|&p| p as *const _).collect();
        SHCreateShellItemArrayFromIDLists(&ids)?.BindToHandler::<_, IDataObject>(None, &BHID_DataObject)
    })();
    for pidl in pidls {
        unsafe { ILFree(Some(pidl)) };
    }
    result
}

/// File paths in a dragged data object (`CF_HDROP`), or none.
pub fn paths_of(data: &IDataObject) -> Vec<PathBuf> {
    let format = FORMATETC {
        cfFormat: CF_HDROP.0,
        ptd: std::ptr::null_mut(),
        dwAspect: DVASPECT_CONTENT.0,
        lindex: -1,
        tymed: TYMED_HGLOBAL.0 as u32,
    };
    unsafe {
        let Ok(mut medium) = data.GetData(&format) else { return Vec::new() };
        let mem = medium.u.hGlobal;
        let p = GlobalLock(mem);
        let paths = if p.is_null() {
            Vec::new()
        } else {
            let bytes = std::slice::from_raw_parts(p as *const u8, GlobalSize(mem));
            let paths = crate::clip::decode_hdrop(bytes);
            let _ = GlobalUnlock(mem);
            paths
        };
        ReleaseStgMedium(&mut medium);
        paths
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: u32 = COPY | MOVE | LINK;

    #[test]
    fn data_objects_carry_the_dragged_paths() {
        let _com = crate::shell::Com::init();
        let d = std::env::temp_dir().join(format!("fep-dnd-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let paths = vec![d.join("one.txt"), d.join("two words.txt")];
        for p in &paths {
            std::fs::write(p, "").unwrap();
        }
        let data = data_object(&paths).unwrap();
        let back: Vec<String> =
            paths_of(&data).iter().map(|p| p.display().to_string().to_lowercase()).collect();
        let want: Vec<String> = paths
            .iter()
            .map(|p| std::fs::canonicalize(p).unwrap())
            .map(|p| p.display().to_string().trim_start_matches(r"\\?\").to_lowercase())
            .collect();
        assert_eq!(back, want);
        std::fs::remove_dir_all(d).unwrap();
    }

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    #[test]
    fn plain_drag_moves_within_a_drive_and_copies_across() {
        let from = [p(r"C:\a\x.txt")];
        assert_eq!(drop_effect(0, ALL, &from, Path::new(r"c:\b")), MOVE);
        assert_eq!(drop_effect(0, ALL, &from, Path::new(r"D:\b")), COPY);
    }

    #[test]
    fn ctrl_copies_and_shift_moves() {
        let from = [p(r"C:\a\x.txt")];
        assert_eq!(drop_effect(MK_CONTROL, ALL, &from, Path::new(r"C:\b")), COPY);
        assert_eq!(drop_effect(MK_SHIFT, ALL, &from, Path::new(r"D:\b")), MOVE);
    }

    #[test]
    fn shortcuts_and_drops_into_the_same_folder_are_refused() {
        let from = [p(r"C:\a\x.txt")];
        assert_eq!(drop_effect(MK_CONTROL | MK_SHIFT, ALL, &from, Path::new(r"C:\b")), NONE);
        assert_eq!(drop_effect(MK_ALT, ALL, &from, Path::new(r"C:\b")), NONE);
        assert_eq!(drop_effect(0, ALL, &from, Path::new(r"C:\a")), NONE);
        assert_eq!(drop_effect(0, ALL, &[p(r"C:\a")], Path::new(r"C:\a\inside")), NONE, "into itself");
    }

    #[test]
    fn falls_back_to_what_the_source_allows() {
        let from = [p(r"C:\a\x.txt")];
        assert_eq!(drop_effect(MK_SHIFT, COPY, &from, Path::new(r"D:\b")), COPY);
        assert_eq!(drop_effect(0, MOVE, &from, Path::new(r"D:\b")), MOVE);
        assert_eq!(drop_effect(0, LINK, &from, Path::new(r"D:\b")), NONE);
        assert_eq!(drop_effect(0, ALL, &[], Path::new(r"D:\b")), NONE);
    }

    #[test]
    fn volumes_compare_by_drive_or_share_ignoring_case() {
        assert!(same_volume(Path::new(r"C:\a"), Path::new(r"c:\b\c")));
        assert!(!same_volume(Path::new(r"C:\a"), Path::new(r"D:\a")));
        assert!(same_volume(Path::new(r"\\srv\share\x"), Path::new(r"\\SRV\Share\y")));
        assert!(!same_volume(Path::new(r"\\srv\share\x"), Path::new(r"\\srv\other\x")));
    }
}
