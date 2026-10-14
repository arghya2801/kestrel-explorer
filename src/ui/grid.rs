//! Virtual grid images: one worker, a bounded queue, and at most 256 retained thumbnails per view.
use super::Event;
use crate::events::Events;
use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicI32, AtomicU64, Ordering},
        mpsc::{self, SyncSender},
    },
};
use windows::Win32::{
    Foundation::SIZE,
    Graphics::Gdi::{BITMAP, DeleteObject, GetObjectW, HBITMAP, HGDIOBJ},
    UI::{
        Controls::*,
        Shell::{IShellItemImageFactory, SHCreateItemFromParsingName, SIIGBF_CROPTOSQUARE, SIIGBF_SCALEUP},
        WindowsAndMessaging::*,
    },
};
use windows::core::HSTRING;

pub(super) struct Bitmap(pub isize);
impl Drop for Bitmap {
    fn drop(&mut self) {
        unsafe {
            let _ = DeleteObject(HGDIOBJ(self.0 as _));
        }
    }
}
struct Request {
    generation: u64,
    entry: u32,
    row: usize,
    path: PathBuf,
}
pub(super) struct Grid {
    pub enabled: bool,
    /// Thumbnail edge at 100% scaling (Settings::thumb_size).
    pub size: i32,
    /// The image list's edge in pixels, which the worker renders at.
    px: Arc<AtomicI32>,
    pub images: HIMAGELIST,
    generation: u64,
    latest: Arc<AtomicU64>,
    visible: Arc<Mutex<(usize, usize)>>,
    tx: Option<SyncSender<Request>>,
    events: Arc<Events<Event>>,
    view: u64,
    pending: HashSet<u32>,
    cache: HashMap<u32, i32>,
    slots: Vec<u32>,
    failed: VecDeque<u32>,
    replace: usize,
}
impl Drop for Grid {
    fn drop(&mut self) {
        self.latest.store(u64::MAX, Ordering::Relaxed);
        unsafe {
            let _ = ImageList_Destroy(Some(self.images));
        }
    }
}
impl Grid {
    pub fn new(enabled: bool, size: i32, view: u64, events: Arc<Events<Event>>) -> Self {
        Self {
            enabled,
            size,
            px: Arc::default(),
            images: HIMAGELIST::default(),
            generation: 0,
            latest: Arc::default(),
            visible: Arc::new(Mutex::new((0, usize::MAX))),
            tx: None,
            events,
            view,
            pending: HashSet::new(),
            cache: HashMap::new(),
            slots: Vec::new(),
            failed: VecDeque::new(),
            replace: 0,
        }
    }
    /// Creates the image list and worker, or rebuilds the list when the size or `dpi` changed.
    pub fn ensure(&mut self, dpi: u32) {
        let px = self.size * dpi as i32 / 96;
        if self.px.swap(px, Ordering::Relaxed) != px && !self.images.is_invalid() {
            unsafe {
                let _ = ImageList_Destroy(Some(self.images));
            }
            self.images = HIMAGELIST::default();
            self.clear();
        }
        if self.images.is_invalid() {
            self.images = unsafe { ImageList_Create(px, px, ILC_COLOR32, 1, 32) };
            self.placeholder();
        }
        if self.tx.is_some() {
            return;
        }
        let (tx, rx) = mpsc::sync_channel::<Request>(64);
        let (latest, visible, events, view, px) =
            (self.latest.clone(), self.visible.clone(), self.events.clone(), self.view, self.px.clone());
        if std::thread::Builder::new()
            .name("thumbnails".into())
            .spawn(move || {
                let _com = crate::shell::Com::init();
                for job in rx {
                    let range = *visible.lock().unwrap_or_else(|e| e.into_inner());
                    let attempted = job.generation == latest.load(Ordering::Relaxed)
                        && (range.0..=range.1).contains(&job.row);
                    let image = if attempted {
                        crate::crash::guard(|| unsafe {
                            let factory: IShellItemImageFactory =
                                SHCreateItemFromParsingName(&HSTRING::from(job.path.as_os_str()), None)
                                    .ok()?;
                            let px = px.load(Ordering::Relaxed);
                            factory
                                .GetImage(SIZE { cx: px, cy: px }, SIIGBF_CROPTOSQUARE | SIIGBF_SCALEUP)
                                .ok()
                                .map(|b| Bitmap(b.0 as isize))
                        })
                        .flatten()
                    } else {
                        None
                    };
                    events.send(Event::Thumbnail {
                        view,
                        generation: job.generation,
                        entry: job.entry,
                        attempted,
                        image,
                    });
                }
            })
            .is_ok()
        {
            self.tx = Some(tx);
        }
    }
    fn placeholder(&self) {
        unsafe {
            if let Ok(icon) = LoadIconW(None, IDI_APPLICATION) {
                ImageList_ReplaceIcon(self.images, -1, icon);
            }
        }
    }
    pub fn hint(&self, from: usize, to: usize) {
        *self.visible.lock().unwrap_or_else(|e| e.into_inner()) = (from, to);
    }
    pub fn icon(&mut self, generation: u64, entry: u32, row: usize, path: PathBuf) -> i32 {
        if generation != self.generation {
            self.generation = generation;
            self.latest.store(generation, Ordering::Relaxed);
            self.clear();
            unsafe {
                let _ = ImageList_Remove(self.images, -1);
            }
            self.placeholder();
        }
        if let Some(&slot) = self.cache.get(&entry) {
            return slot;
        }
        if !self.pending.contains(&entry)
            && let Some(tx) = &self.tx
            && tx.try_send(Request { generation, entry, row, path }).is_ok()
        {
            self.pending.insert(entry);
        }
        0
    }
    fn clear(&mut self) {
        self.pending.clear();
        self.cache.clear();
        self.slots.clear();
        self.failed.clear();
        self.replace = 0;
    }
    /// Retained thumbnails: up to 256, within about 32 MB at large sizes.
    fn capacity(&self) -> usize {
        let px = self.px.load(Ordering::Relaxed).max(1) as usize;
        ((32 << 20) / (px * px * 4)).clamp(32, 256)
    }
    pub fn finish(&mut self, generation: u64, entry: u32, attempted: bool, image: Option<Bitmap>) -> bool {
        if generation != self.generation {
            return false;
        }
        // Rendered before a size change: it would not fit the new image list.
        if let Some(b) = &image {
            let mut bm = BITMAP::default();
            let got = unsafe {
                GetObjectW(HGDIOBJ(b.0 as _), size_of::<BITMAP>() as i32, Some(&mut bm as *mut _ as *mut _))
            };
            if got == 0 || bm.bmWidth != self.px.load(Ordering::Relaxed) {
                self.pending.remove(&entry);
                return true;
            }
        }
        self.pending.remove(&entry);
        if !attempted {
            return true;
        }
        let Some(image) = image else {
            self.cache.insert(entry, 0);
            self.failed.push_back(entry);
            if self.failed.len() > 64
                && let Some(old) = self.failed.pop_front()
            {
                self.cache.remove(&old);
            }
            return true;
        };
        let slot = unsafe {
            if self.slots.len() < self.capacity() {
                let slot = ImageList_Add(self.images, HBITMAP(image.0 as _), None);
                if slot < 0 {
                    return false;
                }
                self.slots.push(entry);
                slot
            } else {
                let old = std::mem::replace(&mut self.slots[self.replace], entry);
                let slot = self.cache.remove(&old).unwrap_or((self.replace + 1) as i32);
                if !ImageList_Replace(self.images, slot, HBITMAP(image.0 as _), None).as_bool() {
                    return false;
                }
                self.replace = (self.replace + 1) % self.slots.len();
                slot
            }
        };
        self.cache.insert(entry, slot);
        true
    }
}
pub(super) fn set(enabled: bool) {
    let Some(list) = super::with_app(|a| a.view.list) else { return };
    let dpi = unsafe { windows::Win32::UI::HiDpi::GetDpiForWindow(list) };
    let Some(images) = super::with_app(|a| {
        a.view.grid.enabled = enabled;
        if enabled {
            a.view.grid.ensure(dpi);
        }
        a.view.grid.images
    }) else {
        return;
    };
    unsafe {
        use windows::Win32::Foundation::{LPARAM, WPARAM};
        if enabled {
            SendMessageW(list, LVM_SETIMAGELIST, Some(WPARAM(LVSIL_NORMAL as usize)), Some(LPARAM(images.0)));
            // Back to the default spacing, which follows the image size.
            SendMessageW(list, LVM_SETICONSPACING, None, Some(LPARAM(-1)));
        }
        let style = GetWindowLongPtrW(list, GWL_STYLE) as u32;
        if style & LVS_TYPEMASK == if enabled { LVS_ICON } else { LVS_REPORT } {
            return;
        }
        SetWindowLongPtrW(
            list,
            GWL_STYLE,
            ((style & !LVS_TYPEMASK) | if enabled { LVS_ICON } else { LVS_REPORT }) as isize,
        );
        let _ = windows::Win32::Graphics::Gdi::InvalidateRect(Some(list), None, true);
    }
}

/// Shows thumbnails at `size` (see `THUMB_SIZES`).
pub(super) fn set_size(size: i32) {
    super::with_app(|a| a.view.grid.size = size);
    set(true);
}

/// Ctrl+mouse wheel: larger thumbnails, or from Details into the grid; smaller ones,
/// and from the smallest back to Details.
pub(super) fn zoom(bigger: bool) {
    use crate::settings::THUMB_SIZES;
    let Some((enabled, size)) = super::with_app(|a| (a.view.grid.enabled, a.view.grid.size)) else { return };
    let i = THUMB_SIZES.iter().position(|t| t.0 >= size).unwrap_or(THUMB_SIZES.len() - 1);
    match (enabled, bigger) {
        (false, true) => set_size(THUMB_SIZES[0].0),
        (false, false) => {}
        (true, false) if i == 0 => set(false),
        (true, false) => set_size(THUMB_SIZES[i - 1].0),
        (true, true) => set_size(THUMB_SIZES[(i + 1).min(THUMB_SIZES.len() - 1)].0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn thumbnail_cache_is_bounded_and_rejects_stale_results() {
        let mut grid = Grid::new(true, 96, 0, Arc::new(Events::new(|| true)));
        grid.ensure(96);
        assert!(!grid.images.is_invalid());
        for entry in 0..300 {
            let bitmap = unsafe { windows::Win32::Graphics::Gdi::CreateBitmap(96, 96, 1, 32, None) };
            assert!(!bitmap.is_invalid());
            assert!(grid.finish(0, entry, true, Some(Bitmap(bitmap.0 as isize))));
        }
        assert_eq!(grid.cache.len(), 256);
        assert_eq!(unsafe { ImageList_GetImageCount(grid.images) }, 257);
        assert!(!grid.cache.contains_key(&0));
        assert!(!grid.finish(1, 301, true, None));
        assert!(!grid.cache.contains_key(&301));
    }
}
