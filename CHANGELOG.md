# Changelog

Notable changes to Kestrel. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow [Semantic Versioning](https://semver.org/).

Add a line under **Unreleased** with each change. When cutting a release, rename that section to the version and paste it into the GitHub release body.

## [Unreleased]

### Added
- View > Group by date shows a folder in Explorer's date groups (Today, Yesterday, Earlier this week, Last week…), keeping your sort inside each group. Each folder remembers it, so Downloads can stay grouped while other folders don't.
- A + button after the last tab opens a new tab.
- Duplicate tab, from a tab's right-click menu or Ctrl+Shift+K.
- View > Show in sidebar (Ctrl+Shift+E) expands the sidebar to the open folder.

### Changed
- The sidebar no longer expands to every folder you open; use Show in sidebar when you want it.

### Fixed
- Shift+mouse wheel scrolls the file list sideways.
- Sorting by size uses the folder sizes from View > Calculate folder sizes, and re-sorts when they finish.

## [1.2.1] - 2026-10-15

### Added
- Middle-click a tab to close it.

### Changed
- A redesigned top bar: flat buttons with icons, a rounded address bar whose breadcrumbs fit their names, a search-style filter box, toolbar groups with separators, and tabs that open into the folder below. Focused text boxes show a line in your Windows accent color. Light and dark themes both get the new look; high contrast keeps Windows' own drawing.

## [1.2.0] - 2026-10-15

1.1.0 was never released, so this release also carries its changes.

### Added
- Two independent folder panes in one window. Ctrl+Shift+S toggles the split and F6 focuses the other pane. Each tab remembers its panes.
- View > Sync pane navigation repeats entering a subfolder or going up in the other pane. View > Compare panes selects the files each pane lacks or has with another size or date.
- A command toolbar below the address bar, with narrow-window overflow under More.
- Settings pages for appearance, file list, startup and window, and an F1 keyboard shortcut guide.
- Breadcrumb navigation, Quick access pins, recent folders, Open terminal here and a File operations status window.
- Ctrl+Z and Ctrl+Y undo and redo copies, moves, renames, new folders and deletes, including restoring from the Recycle Bin.
- Batch rename: F2 on several items opens find/replace with `#` numbering, a live preview and a single undo.
- A thumbnail grid in four sizes. Ctrl+mouse wheel changes the size, and thumbnails scale with the display.
- Date created and Attributes columns, shown or hidden from a right-click on the column headers.
- View > Calculate folder sizes fills the Size column for folders. Esc stops it.
- The filter understands `size:>10MB`, `size:<1KB`, `modified:<7d` and `modified:>2w`. Subfolder search also accepts `content:word`.
- .zip files open as folders through Windows' built-in zip support.
- The status bar shows the selection count and size, and the drive's free space.
- Alt+Enter opens Properties.
- Your own key bindings in `keys.txt`, listed first in the F1 guide.
- Portable mode: an empty file named `portable` beside kestrel.exe keeps all data in a KestrelData folder there.
- An optional "Open in Kestrel" entry on folder and drive menus, set on the Settings Startup page.
- Help > Check for updates opens the releases page. Kestrel sends nothing on its own.
- Screen readers get names for the address bar, filter, file lists, sidebar and tab strip. Windows high-contrast mode uses the contrast colors.

### Changed
- Tabs, panes and windows are saved every 30 seconds and at sign-out or restart, so they come back after a crash.
- First paint is faster: the median release launch went from 99 ms to 79 ms on the same machine.
- Thumbnails load on bounded workers with a cache, and off-screen icon requests are cancelled.
- The sidebar follows the current folder and pages branches with thousands of children.

### Fixed
- Properties in Windows' full context menu failed silently because the helper process never initialized OLE.
- Starting Kestrel with a quoted drive root such as `"C:\"` now opens that drive.
- Folders restored at startup are queued instead of dropped when all listing workers are busy.
- Dark-mode column headings and pane labels are readable.
- Pressing Back or Forward twice while a folder is still loading moves two steps, not one.
- Network computers with no shares list as empty instead of failing.

## [1.0.0] - 2026-10-10

First release: tabs, side-by-side windows, previews, filtering and search, dark mode, network browsing, drag and drop, and Windows' full context menu in a helper process.
