//! Network locations: `\\server` lists that computer's shared folders.

use std::io;
use std::path::{Path, PathBuf};

use windows::Win32::NetworkManagement::NetManagement::NetApiBufferFree;
use windows::Win32::Storage::FileSystem::{
    FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_HIDDEN, NetShareEnum, SHARE_INFO_1, STYPE_DISKTREE, STYPE_MASK,
};
use windows::core::HSTRING;

use crate::listing::{Snapshot, list_dir};

/// "srv" for `\\srv` (a computer, not a share or a path inside one).
pub fn server_of(path: &Path) -> Option<String> {
    let s = path.to_string_lossy();
    if s.starts_with(r"\\?\") || s.starts_with(r"\\.\") {
        return None;
    }
    let rest = s.strip_prefix(r"\\")?.trim_end_matches('\\');
    (!rest.is_empty() && !rest.contains('\\')).then(|| rest.to_string())
}

/// The folder above `dir`; above a share's root (`\\srv\share`) is its computer, and above
/// a drive's root is This PC.
pub fn up_from(dir: &Path) -> Option<PathBuf> {
    // A computer is the top. Rust reads a bare `\\srv` as a folder "srv" under the root `\`.
    if server_of(dir).is_some() || crate::places::is_this_pc(dir) {
        return None;
    }
    if let Some(parent) = dir.parent() {
        return Some(parent.to_path_buf());
    }
    if let Some(std::path::Component::Prefix(p)) = dir.components().next()
        && matches!(p.kind(), std::path::Prefix::Disk(_))
    {
        return Some(crate::places::this_pc());
    }
    let s = dir.to_string_lossy();
    let rest = s.strip_prefix(r"\\").filter(|_| !s.starts_with(r"\\?\"))?;
    let (server, share) = rest.trim_end_matches('\\').split_once('\\')?;
    (!share.is_empty() && !share.contains('\\')).then(|| PathBuf::from(format!(r"\\{server}")))
}

/// The disk shares on `server`. Admin shares (`C$`) are marked hidden, like
/// hidden files, so the hidden-items toggle applies. Can wait on the network:
/// worker threads only.
pub fn list_shares(server: &str) -> io::Result<Snapshot> {
    let mut buf: *mut u8 = std::ptr::null_mut();
    let (mut read, mut total) = (0u32, 0u32);
    let rc =
        unsafe { NetShareEnum(&HSTRING::from(server), 1, &mut buf, u32::MAX, &mut read, &mut total, None) };
    let free = |b: *mut u8| {
        if !b.is_null() {
            unsafe { NetApiBufferFree(Some(b as *const _)) };
        }
    };
    if rc != 0 {
        free(buf);
        return Err(io::Error::from_raw_os_error(rc as i32));
    }
    let mut snap = Snapshot::new(PathBuf::from(format!(r"\\{server}")));
    if read == 0 {
        free(buf);
        return Ok(snap);
    }
    if buf.is_null() {
        return Err(io::Error::other("Windows returned a missing share buffer"));
    }
    let items = unsafe { std::slice::from_raw_parts(buf as *const SHARE_INFO_1, read as usize) };
    for item in items {
        if item.shi1_type.0 & STYPE_MASK.0 != STYPE_DISKTREE.0 || item.shi1_netname.is_null() {
            continue; // printers, IPC$
        }
        let name = unsafe { item.shi1_netname.as_wide() };
        let hidden = if name.last() == Some(&(b'$' as u16)) { FILE_ATTRIBUTE_HIDDEN.0 } else { 0 };
        snap.add(name, FILE_ATTRIBUTE_DIRECTORY.0 | hidden, 0, 0);
    }
    free(buf);
    Ok(snap)
}

/// Lists a folder, a computer's shares for `\\server`, This PC's drives, or the inside of a .zip.
pub fn list_location(dir: &Path) -> io::Result<Snapshot> {
    match server_of(dir) {
        Some(server) => list_shares(&server),
        None if crate::places::is_this_pc(dir) => crate::places::list_this_pc(),
        None if crate::shell::in_archive(dir) => crate::shell::list_archive(dir),
        None => list_dir(dir),
    }
}

/// Network browser discovery can be unavailable; callers must keep typed UNC paths usable.
/// https://learn.microsoft.com/windows/win32/api/lmserver/nf-lmserver-netserverenum
pub fn computers() -> io::Result<Vec<String>> {
    use windows::Win32::NetworkManagement::NetManagement::*;
    use windows::core::PCWSTR;
    let mut buffer = std::ptr::null_mut();
    let (mut read, mut total) = (0, 0);
    let rc = unsafe {
        NetServerEnum(
            PCWSTR::null(),
            100,
            &mut buffer,
            u32::MAX,
            &mut read,
            &mut total,
            SV_TYPE_WORKSTATION | SV_TYPE_SERVER,
            PCWSTR::null(),
            None,
        )
    };
    let result = if rc != 0 {
        Err(io::Error::from_raw_os_error(rc as i32))
    } else if read == 0 {
        Ok(Vec::new())
    } else if buffer.is_null() {
        Err(io::Error::other("Windows returned a missing computer buffer"))
    } else {
        let entries = unsafe { std::slice::from_raw_parts(buffer as *const SERVER_INFO_100, read as usize) };
        let mut names: Vec<_> = entries
            .iter()
            .filter(|e| !e.sv100_name.is_null())
            .filter_map(|e| unsafe { e.sv100_name.to_string().ok() })
            .collect();
        names.sort_unstable_by_key(|n| n.to_lowercase());
        names.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
        Ok(names)
    };
    if !buffer.is_null() {
        unsafe {
            NetApiBufferFree(Some(buffer as *const _));
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_server_paths_are_recognised() {
        assert_eq!(server_of(Path::new(r"\\srv")).as_deref(), Some("srv"));
        assert_eq!(server_of(Path::new(r"\\srv\")).as_deref(), Some("srv"));
        assert_eq!(server_of(Path::new(r"\\srv\share")), None);
        assert_eq!(server_of(Path::new(r"C:\x")), None);
        assert_eq!(server_of(Path::new(r"\\?\C:\x")), None);
    }

    #[test]
    fn up_from_a_share_root_goes_to_its_server() {
        assert_eq!(up_from(Path::new(r"\\srv\share")), Some(PathBuf::from(r"\\srv")));
        assert_eq!(up_from(Path::new(r"\\srv\share\dir")), Some(PathBuf::from(r"\\srv\share")));
        assert_eq!(up_from(Path::new(r"C:\a")), Some(PathBuf::from(r"C:\")));
        assert_eq!(up_from(Path::new(r"C:\")), Some(crate::places::this_pc()));
        assert_eq!(up_from(&crate::places::this_pc()), None);
        assert_eq!(up_from(Path::new(r"\\srv")), None);
    }

    #[test]
    #[ignore = "requires localhost SMB access and a C$ administrative share"]
    fn this_computers_disk_shares_are_listed() {
        let all = list_shares("localhost").unwrap();
        let names: Vec<String> = (0..all.len()).map(|i| all.name_string(i)).collect();
        assert!(names.iter().any(|n| n.eq_ignore_ascii_case("C$")), "{names:?}");
        assert!(!names.iter().any(|n| n.eq_ignore_ascii_case("IPC$")), "not a disk share: {names:?}");
        assert!((0..all.len()).all(|i| all.is_dir(i)));
        assert!((0..all.len()).all(|i| all.is_hidden(i) == all.name_string(i).ends_with('$')));
    }

    #[test]
    fn unknown_servers_fail_cleanly() {
        assert!(list_shares("no-such-host.invalid").is_err());
    }
}
