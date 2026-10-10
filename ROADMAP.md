# Kestrel roadmap

v1 goal: Kestrel, a small, fast Windows file explorer that never crashes and never shows "Not Responding".
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
| M6 | Live updates: folder watcher, quiet refresh, deleted folder climbs to parent, USB eject, drive changes |
| M7 | Delete/recycle, rename in place, new folder, copy/cut/paste with Explorer, right-click menu |
| M8 | Settings, hang watchdog, startup timing, exit waits for copies, icon and version, stress test |

Notes from building it:

- Opening a file never shows a modal error; only "How do you want to open this file?" remains.
- Icons are requested while painting, which already limits lookups to visible rows,
  so `LVN_ODCACHEHINT` wasn't needed.
- Renames use the path captured when the edit box opens; list updates wait until it closes,
  so a refresh can never rename the wrong file.
- Closing the window while files are copying keeps the process alive until the copy finishes.
- Shortcut arrow overlays are skipped: overlay queries run every installed overlay
  handler (cloud sync, Git), a known source of slowness.
- The shell's image list setup races between threads on first use; it runs once per process.

## v1.0 measurements

- First paint 54-70 ms after process start (80 ms on a cold first launch). The window and
  controls paint first; the shell icon list and sidebar load right after.
- Release exe 632 KB; about 23 MB of memory with icons loaded.
- Stress test (`cargo run --release --example stress -- 600`): 10 minutes, about 12,000 UI actions
  and 210,000 file changes. Handle, GDI and USER object counts flat. Worst UI stall 119 ms,
  spent with the UI thread not running (antivirus scanning the churned files).
- Open finding: one 5.4 s stall in the first stress run, while a release build and five app
  launches ran alongside. Not reproduced in six later runs (about 27 minutes, one of them
  during a release build). The hang watchdog writes a minidump if it ever happens again.

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
