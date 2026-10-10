# Roadmap

v1 goal: a small, fast Windows file explorer that never crashes and never shows "Not Responding".
Each stage ends with something runnable, its tests green, and a commit per step.

Rules that hold for every stage:

- The UI thread never touches disk, network or shell. Workers report through `Events`.
- Logic goes in plain modules with unit tests written first. `ui.rs` stays a thin layer,
  covered by tests in `tests/window.rs` that drive the real window.
- A test has to be able to fail. Check the starting state differs from the expected one.
- No new dependency unless a few lines of our own code can't do the job.

## Done

| Stage | What |
|---|---|
| M0 | Window, manifest, crash guard, panic log, minidumps, no "insert disk" dialogs |
| M1 | Listing (`FindFirstFileExW`), long paths, natural sort with sort keys, size/date text |
| M2 | Virtual ListView, worker listing/sorting, generations, sort arrows, type-to-find, status bar |
| M3 | History, address bar, Esc cancels stalled loads, opening files and folder shortcuts, Ctrl+H, F5, selection kept |
| M4 | Sidebar: Quick access, drives with labels, lazy folder tree, splitter, Tab order |
| M5 | System icons (per extension; per file for programs/shortcuts), Type column and sort |

Notes from building M3 to M5:

- Opening a file never shows a modal error; only "How do you want to open this file?" remains.
- Icons are requested while painting, which already limits lookups to visible rows,
  so `LVN_ODCACHEHINT` wasn't needed.
- Shortcut arrow overlays are skipped: overlay queries run every installed overlay
  handler (cloud sync, Git), a known source of slowness.
- The shell's image list setup races between threads on first use; it runs once per process.

## M3: navigation and opening files

- Back/Forward history in `Browser`. Alt+Left/Right, mouse X1/X2 buttons.
  Backspace changes from "up" to "back" to match Explorer. Alt+Up goes up.
- Address bar (Edit control above the list). Enter navigates, Esc restores, Ctrl+L/Alt+D/F4 focus.
  Input is normalized as a string only: quotes trimmed, `%VARS%` expanded, made absolute.
- After Back or Up, select the folder you came from. The same select-by-name keeps the
  selection across a re-sort, replacing the current "selection clears" behavior.
- Open files with `ShellExecuteExW` on a worker thread. Errors go to the status bar, not dialogs.
  A `.lnk` that points to a folder opens inside the app.
- Esc while loading cancels: `CancelSynchronousIo` on the worker, then the generation is dropped.
  At most 8 stalled workers; past that, new network navigations are refused with a message.
- Ctrl+H toggles hidden files (re-filter on a worker, no re-listing). F5 re-lists.
- Tests: history, input normalization, cancel, select-by-name (unit);
  address bar navigation and Back/Forward (window).

## M4: sidebar

- TreeView on the left: Quick access (Desktop, Downloads, Documents, Pictures, Music, Videos
  from `SHGetKnownFolderPath`) and This PC (`GetLogicalDrives` + `GetDriveTypeW`).
- Drive labels and child folders load on workers. Optical, removable and network drives can block.
- Folders expand lazily: every node starts expandable; the worker lists sub-folders on expand.
- Draggable splitter between tree and list.
- Not in v1: auto-expanding the tree to follow the current folder.
- Tests: drive-type mapping, splitter clamping (unit); expanding a node fills it (window).

## M5: icons and the Type column

- One STA icon thread. `SHGetFileInfoW` with `SYSICONINDEX | USEFILEATTRIBUTES | TYPENAME`,
  cached per extension, so most files never get opened.
- Real per-file icons only for `.exe .ico .lnk .url .cpl`, only for rows on screen (`LVN_ODCACHEHINT`).
- System image list shared by the ListView and TreeView.
- Type column. Sorting by type uses the lowercase extension, which needs no shell call.
- Tests: extension parsing (no extension, dotfiles, multiple dots), cache, stale results dropped.

## M6: live updates

Comes before file operations so that operations need no refresh code: the watcher shows their results,
and changes made by other programs show up the same way.

- Watcher thread for the current folder: `ReadDirectoryChangesW`, 64 KB buffer,
  `FILE_SHARE_DELETE` so we never block renames or deletes, stopped with `CancelIoEx`.
- Any change, overflow or `ERROR_NOTIFY_ENUM_DIR`: wait 100 ms for more, then re-list.
  Selection, focus and scroll position are kept by name.
- Folder deleted while open: go to the nearest parent that still exists.
- USB eject: close the watcher on `DBT_DEVICEQUERYREMOVE` so "Safely remove" works.
  Refresh the drive list on device arrival and removal.
- Tests: debounce with a fake clock (unit); a file created in the open folder appears,
  and deleting the open folder moves to its parent (window).

## M7: file operations

- One STA operations thread using `IFileOperation`: delete to Recycle Bin (Shift+Del deletes
  permanently), rename, new folder, copy, move. Windows shows its own progress and conflict dialogs.
- Multi-select (`LVN_ODSTATECHANGED`).
- Rename in place with F2. Names are validated before the operation runs.
- Clipboard: Ctrl+C/X/V with `CF_HDROP` + `Preferred DropEffect`, so copy and paste work
  with Windows Explorer in both directions.
- Our own right-click menu: Open, Cut, Copy, Paste, Rename, Delete, New folder, Copy as path,
  Show in Explorer. The last one opens Explorer with the item selected, which gives access to the
  full shell menu and Properties without loading third-party shell extensions into our process.
- New and renamed items get selected once the watcher re-lists.
- Not in v1: Undo.
- Tests: name validation, `CF_HDROP` round trip (unit); rename, new folder, permanent delete,
  clipboard copy/paste in a temp folder (window).

## M8: hardening and v1.0

- Debug-build watchdog: if the UI thread doesn't answer within 2 s, write a minidump.
- Stress example: files churning plus random navigation every 50 ms for 30 minutes,
  with handle, GDI and USER object counts flat.
- Settings: window position, column widths, sort, hidden-files toggle, last folder. Stored as
  `key=value` in `%LOCALAPPDATA%`, written to a temp file then renamed, so a crash can't corrupt it.
  A bad file means defaults.
- Startup time logged to first paint. Target under 100 ms.
- App icon and version info.
- Run the test matrix: empty card reader, `System Volume Information`, junctions,
  paths over 260 characters, disconnected mapped drive, folder deleted while open,
  200k+ items, odd file names.
- Release build, tag `v1.0`.

## After v1, in order

1. Drag and drop with other apps (OLE, both directions).
2. Full shell context menu, hosted in a helper process so a buggy extension can't take us down.
3. Filter the current folder as you type (in memory, instant), then recursive search on a worker.
4. Tabs.
5. Dark mode.
6. Preview pane and thumbnails, out of process for the same reason as 2.
7. Dual pane.
8. Browsing network computers and their shares.

Not planned: plugins, cloud-drive integrations, built-in archive browsing,
an in-process Properties dialog (Show in Explorer covers it).
