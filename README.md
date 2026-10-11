<p align="center"><img src="docs/icon.png" width="128" alt="Kestrel icon: a yellow folder with a gliding kestrel"></p>

<h1 align="center">Kestrel</h1>

<p align="center">A small, fast Windows file explorer that doesn't freeze.<br>
Rust and plain Win32, one 800 KB exe, no runtime.</p>

---

Kestrel opens in about 150 ms, uses around 25 MB of memory, and keeps its window responsive while
a network drive is dead, a folder holds 200,000 files, or a copy is running. Everything that can
block (disk, network, shell extensions) runs off the UI thread.

## Features

- Fast folder listing with Explorer-style natural sort ("file2" before "file10"), folders first
- Address bar, Back / Forward / Up, sidebar with Quick access and drives
- Live updates when files change on disk
- Copy, cut, paste, rename, delete to Recycle Bin, new folder, all through Windows' own file
  operations, and the clipboard works both ways with Explorer
- Drag and drop with other apps
- Filter the current folder as you type, or search all subfolders
- Tabs, plus side-by-side windows
- Preview pane for text, images and anything with a Windows preview handler
- Windows' full right-click menu (Shift+right-click), run in a helper process
- Dark mode that follows Windows
- `\\server` in the address bar lists a computer's shared folders
- Remembers window size, columns, sort, sidebar width and last folder

## Keyboard

| Keys | Action |
|---|---|
| Enter, double-click | Open |
| Backspace, Alt+Left / Alt+Right | Back / Forward |
| Alt+Up | Up one folder |
| Ctrl+L, Alt+D, F4 | Address bar |
| Ctrl+F | Filter; Enter in the filter searches subfolders |
| Esc | Stop loading or searching, clear the filter |
| F5 | Refresh |
| Ctrl+H | Show hidden items |
| F2 | Rename |
| Del / Shift+Del | Recycle / delete permanently |
| Ctrl+C / Ctrl+X / Ctrl+V | Copy / cut / paste |
| Ctrl+Shift+N | New folder |
| Ctrl+Shift+C | Copy as path |
| Ctrl+A | Select all |
| Ctrl+T / Ctrl+W / Ctrl+Tab | New tab / close tab / next tab |
| Ctrl+N | New window, side by side |
| Alt+P | Preview pane |
| Shift+right-click | Windows' full right-click menu |

## How it stays responsive

- **The UI thread never waits on disk, network or the shell.** Listings, searches, icons, file
  operations and clipboard reads run on worker threads and report back through a message queue.
  A folder on a dead network share shows "Loading…", and Esc abandons it.
- **Third-party shell extensions stay out of the process.** Right-click menu handlers run in a
  short-lived helper copy of Kestrel, and preview handlers in Windows' own preview host. A buggy
  extension can crash those, not your window.
- **Crashes are caught and recorded.** Panics in callbacks are contained, native crashes write a
  minidump, and a watchdog records a dump if the window ever stops responding
  (`%LOCALAPPDATA%\Kestrel\crashes`).
- **Moves are never half done.** Closing the window while files are copying keeps the process
  alive until the copy finishes.

## Build

Requires Windows 10/11, Rust (stable, MSVC toolchain) and the Visual Studio Build Tools.

```
cargo build --release
target\release\kestrel.exe [folder]
```

## Tests

```
cargo test
```

Unit tests cover the logic (listing, sorting, navigation, file operations, clipboard, search,
settings…). `tests/window.rs` drives the real window: it sends the input a user would and waits
for the background work to show up.

The localhost administrative-share tests are opt-in: run `cargo test -- --ignored` on a machine
with localhost SMB access and a readable `C<p align="center"><img src="docs/icon.png" width="128" alt="Kestrel icon: a yellow folder with a gliding kestrel"></p>

<h1 align="center">Kestrel</h1>

<p align="center">A small, fast Windows file explorer that doesn't freeze.<br>
Rust and plain Win32, one 800 KB exe, no runtime.</p>

---

Kestrel opens in about 150 ms, uses around 25 MB of memory, and keeps its window responsive while
a network drive is dead, a folder holds 200,000 files, or a copy is running. Everything that can
block (disk, network, shell extensions) runs off the UI thread.

## Features

- Fast folder listing with Explorer-style natural sort ("file2" before "file10"), folders first
- Address bar, Back / Forward / Up, sidebar with Quick access and drives
- Live updates when files change on disk
- Copy, cut, paste, rename, delete to Recycle Bin, new folder, all through Windows' own file
  operations, and the clipboard works both ways with Explorer
- Drag and drop with other apps
- Filter the current folder as you type, or search all subfolders
- Tabs, plus side-by-side windows
- Preview pane for text, images and anything with a Windows preview handler
- Windows' full right-click menu (Shift+right-click), run in a helper process
- Dark mode that follows Windows
- `\\server` in the address bar lists a computer's shared folders
- Remembers window size, columns, sort, sidebar width and last folder

## Keyboard

| Keys | Action |
|---|---|
| Enter, double-click | Open |
| Backspace, Alt+Left / Alt+Right | Back / Forward |
| Alt+Up | Up one folder |
| Ctrl+L, Alt+D, F4 | Address bar |
| Ctrl+F | Filter; Enter in the filter searches subfolders |
| Esc | Stop loading or searching, clear the filter |
| F5 | Refresh |
| Ctrl+H | Show hidden items |
| F2 | Rename |
| Del / Shift+Del | Recycle / delete permanently |
| Ctrl+C / Ctrl+X / Ctrl+V | Copy / cut / paste |
| Ctrl+Shift+N | New folder |
| Ctrl+Shift+C | Copy as path |
| Ctrl+A | Select all |
| Ctrl+T / Ctrl+W / Ctrl+Tab | New tab / close tab / next tab |
| Ctrl+N | New window, side by side |
| Alt+P | Preview pane |
| Shift+right-click | Windows' full right-click menu |

## How it stays responsive

- **The UI thread never waits on disk, network or the shell.** Listings, searches, icons, file
  operations and clipboard reads run on worker threads and report back through a message queue.
  A folder on a dead network share shows "Loading…", and Esc abandons it.
- **Third-party shell extensions stay out of the process.** Right-click menu handlers run in a
  short-lived helper copy of Kestrel, and preview handlers in Windows' own preview host. A buggy
  extension can crash those, not your window.
- **Crashes are caught and recorded.** Panics in callbacks are contained, native crashes write a
  minidump, and a watchdog records a dump if the window ever stops responding
  (`%LOCALAPPDATA%\Kestrel\crashes`).
- **Moves are never half done.** Closing the window while files are copying keeps the process
  alive until the copy finishes.

## Build

Requires Windows 10/11, Rust (stable, MSVC toolchain) and the Visual Studio Build Tools.

```
cargo build --release
target\release\kestrel.exe [folder]
```

## Tests

```
cargo test
```

Unit tests cover the logic (listing, sorting, navigation, file operations, clipboard, search,
settings…). `tests/window.rs` drives the real window: it sends the input a user would and waits
for the background work to show up.

 share. Clipboard tests use the desktop clipboard;
focus-dependent tests should run while the desktop is idle.

Other tools:

```
cargo run --release --example bench -- C:\Windows\System32   # listing and sorting speed
cargo run --release --example stress -- 600                  # 10 minutes of churn and random use
```

## Layout

| Path | What |
|---|---|
| `src/listing.rs`, `sort.rs`, `search.rs` | Reading folders, ordering, searching |
| `src/browser.rs` | Navigation state and history, independent of any window |
| `src/ops.rs`, `clip.rs`, `dnd.rs` | File operations, clipboard, drag and drop |
| `src/watch.rs` | Folder change watcher |
| `src/ui.rs`, `src/ui/` | The window: list, sidebar, tabs, preview pane, menus, dark mode |
| `res/make_icon.py` | Draws the app icon |

[ROADMAP.md](ROADMAP.md) has the design decisions and historical measurements.
[HANDOFF.md](HANDOFF.md) records known gaps; [PLAN.md](PLAN.md) organizes the remaining work and planned features.
