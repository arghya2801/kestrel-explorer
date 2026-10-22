# Changelog

Notable changes to Kestrel. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow [Semantic Versioning](https://semver.org/).

Add a line under **Unreleased** with each change. When cutting a release, rename that section to the version and paste it into the GitHub release body.

## [Unreleased]

### Added
- This PC: click it in the sidebar, or type it in the address bar, for a page listing the drives with their type, free space and size. Up from a drive's root goes there, and a drive that disappears while open falls back to it. (#19)
- Tabs above the sidebar: View > Tabs above the sidebar, or a tab's right-click menu, lists the tabs in a column above the sidebar instead of along the top. Each window remembers the choice. (#16)
- Split panes: Open in other pane for folders in the list and the sidebar (it splits the view if needed); Copy to other pane (Shift+F5) and Move to other pane (Shift+F6); View > Swap panes (Ctrl+U); View > Show this folder in the other pane (Ctrl+Shift+U). (#17)
- Pin to Quick access and Unpin from Quick access on the right-click menus of the file list and the sidebar. The sidebar's menu also opens a folder in a new tab, in the other pane, or its Properties. (#15)
- Hovering a tab or a sidebar item shows its full path. (#18)

### Changed
- Tabs on different folders of the same name add the parent folders that tell them apart ("MFA221 · bin", "MFA221 · target"). Quick access names pinned folders the same way instead of by their full path. (#18)
- Desktop, Downloads and the other standard folders can be unpinned from Quick access. (#15)
- The active split pane's path label is tinted with the Windows accent color, and the other pane's is dimmed. (#17)

## [1.3.0] - 2026-10-21

### Added
- View > Group by date shows a folder in Explorer's date groups (Today, Yesterday, Earlier this week, Last week…), keeping your sort inside each group. Each folder remembers it, so Downloads can stay grouped while other folders don't.
- A + button after the last tab opens a new tab.
- Duplicate tab, from a tab's right-click menu or Ctrl+Shift+K.
- View > Show in sidebar (Ctrl+Shift+E) expands the sidebar to the open folder.

### Changed
- Each folder remembers its own sort. Folders you haven't sorted use the default sort from Settings > File list, instead of whatever sort you last picked elsewhere.
- The sidebar no longer expands to every folder you open; use Show in sidebar when you want it.
- Column lines in Details view are a faint 1px line instead of the list theme's heavy 2px one.

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
