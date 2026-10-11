//! Virtual grid images: one worker, a bounded queue, and at most 256 retained thumbnails per view.
use super::Event;
use crate::events::Events;
use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
        mpsc::{self, SyncSender},
    },
};
use windows::Win32::{
    Foundation::SIZE,
    Graphics::Gdi::{DeleteObject, HBITMAP, HGDIOBJ},
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
    pub fn new(enabled: bool, view: u64, events: Arc<Events<Event>>) -> Self {
        Self {
            enabled,
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
    pub fn ensure(&mut self) {
        if self.images.is_invalid() {
            self.images = unsafe { ImageList_Create(96, 96, ILC_COLOR32, 1, 32) };
            self.placeholder();
        }
        if self.tx.is_some() {
            return;
        }
        let (tx, rx) = mpsc::sync_channel::<Request>(64);
        let (latest, visible, events, view) =
            (self.latest.clone(), self.visible.clone(), self.events.clone(), self.view);
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
                            factory
                                .GetImage(SIZE { cx: 96, cy: 96 }, SIIGBF_CROPTOSQUARE | SIIGBF_SCALEUP)
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
            self.pending.clear();
            self.cache.clear();
            self.slots.clear();
            self.failed.clear();
            self.replace = 0;
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
    pub fn finish(&mut self, generation: u64, entry: u32, attempted: bool, image: Option<Bitmap>) -> bool {
        if generation != self.generation {
            return false;
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
            if self.slots.len() < 256 {
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
                self.replace = (self.replace + 1) % 256;
                slot
            }
        };
        self.cache.insert(entry, slot);
        true
    }
}
pub(super) fn set(enabled: bool) {
    let Some((list, images)) = super::with_app(|a| {
        a.view.grid.enabled = enabled;
        if enabled {
            a.view.grid.ensure();
        }
        (a.view.list, a.view.grid.images)
    }) else {
        return;
    };
    unsafe {
        use windows::Win32::Foundation::{LPARAM, WPARAM};
        if enabled {
            SendMessageW(list, LVM_SETIMAGELIST, Some(WPARAM(LVSIL_NORMAL as usize)), Some(LPARAM(images.0)));
        }
        let style = GetWindowLongPtrW(list, GWL_STYLE) as u32;
        SetWindowLongPtrW(
            list,
            GWL_STYLE,
            ((style & !LVS_TYPEMASK) | if enabled { LVS_ICON } else { LVS_REPORT }) as isize,
        );
        let _ = windows::Win32::Graphics::Gdi::InvalidateRect(Some(list), None, true);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn thumbnail_cache_is_bounded_and_rejects_stale_results() {
        let mut grid = Grid::new(true, 0, Arc::new(Events::new(|| true)));
        grid.ensure();
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
