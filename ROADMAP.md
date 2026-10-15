# Kestrel roadmap

[HANDOFF.md](HANDOFF.md) records validation commands and remaining release checks.

## Built through 1.2.1

| Area | Implemented |
|---|---|
| Files | Natural sorting, virtual lists, live updates, file operations, clipboard and drag/drop |
| Navigation | History, address editing, breadcrumbs, filter/search, tabs, independent folder panes and windows |
| Sidebar | Quick access pins, drives, Network, active-folder synchronization and child paging |
| View | Details, bounded thumbnail grid, light/dark themes, resizable text/image/shell previews |
| Controls | Toolbar with overflow, Settings pages, shortcut guide, recent folders and terminal launch |
| Persistence | Validated atomic preferences, normal-exit tab/window sessions and missing-folder fallback |
| Diagnostics | Operation status, crash logs/dumps, watchdog, startup probe and expanded stress runner |

Latest release 1.2.1; see CHANGELOG.md.

## Current measurements, 2026-10-15 (first paint, size, tests) and 2026-10-11 (the rest)

| Measurement | Result |
|---|---|
| Release first paint, seven launches | 222, 103, 70, 79, 72, 70, 93 ms; median 79 ms (2026-10-11: median 99 ms) |
| Release executable | 1,264,128 bytes (2026-10-11: 1,089,024) |
| Working set at first paint | 14,208 to 14,764 KiB; not steady-state memory |
| Restore 50,001 selected names in 100,001 items | 26.44 ms in a release logic test |
| Last-visible icon behind 250 executable requests | 7.52 s before viewport cancellation; 11.56 ms after |
| Applicable regression suite | 143 unit tests, 50 window tests and 1 key-binding test passed |
| Expanded 600-second stress | 7,942 actions; 210,141 file changes; worst interval 1.585 s |
| Resource change versus warm-up layout | Handles -44, GDI +7, USER -5; focused GDI follow-up below |

These are measurements on one Windows desktop. No controlled cold-cache run has been made.
The startup comparison immediately before optimization measured 114 to 126 ms warm.
Moving drag-target creation and registration after first paint removed an 18 to 23 ms startup stage.
Preview control creation measured 2 to 4 ms and theme application 0 to 1 ms, so both stayed in place.
DLL delay-loading was not justified by the measurements.


A 120-second follow-up used the same light theme and settled grid state at each resource sample.
It completed 1,483 actions and 42,405 file changes. Samples of handles/GDI/USER were
(528,80,58), (525,80,58), (528,79,58), and final (523,79,56), giving changes -5/-1/-2.
No sustained resource growth was observed. This supports state variation as the explanation for
the earlier +7 GDI result, rather than a demonstrated leak. Worst interval was 1.542 seconds.
The stress runner now records repeated comparable samples; no application code changed for this follow-up.

The icon diagnostic copies the same executable 250 times and warms the final icon in both variants.
The expanded stress runner mixes navigation, search/filter, tabs, split panes, preview/grid,
Settings/theme changes and secondary windows while files change on disk. It compares resources
after returning to the same layout at warm-up and completion.

## Historical v1.0 measurements

- First paint took 54 to 70 ms warm, with an 80 ms first launch recorded at the time.
- The release executable was about 632 KB; reported memory with icons loaded was about 23 MB.
- A 600-second stress run performed about 12,000 UI actions and 210,000 file changes.
  Handle/GDI/USER counts stayed flat; the worst recorded stall was 119 ms.
- A separate initial run recorded one 5.4-second stall during concurrent builds and launches.
  Its cause is still unknown. Later successful runs do not close that finding.

The post-v1 handoff recorded 141 to 196 ms warm startup before this pass.
Historical samples used different feature sets and workload conditions.

## Design decisions

- Keep disk, network and shell work on workers and route results through Events.
- Every pane owns its navigation and controls. Tab switches retain pane state.
- View identity plus generation rejects stale asynchronous results.
- File operations capture source/destination paths when the command starts.
- Closing windows keeps the process alive until pending file operations finish.
- Eight folder-listing slots bound concurrent work; each pane retains its latest queued request.
- Thumbnail work skips offscreen rows, uses a bounded queue and retains at most 256 images per view.
- Per-file icon workers discard offscreen requests; extension and sidebar icons still share existing caches.
- Full shell menus run in a helper process and preview handlers use Windows' preview host.
- Preferences and sessions have coordinated ownership, atomic writes and legacy settings migration.
- Native menus remain light; undocumented uxtheme ordinals are not used.
- Shortcut-arrow overlays remain deferred because installed handlers can be slow.

## Road to 2.0 (final product)

Review of 2026-10-14; work through 2026-10-15 on branch roadmap-2.0.
Items are ordered by priority inside each phase.

### 1.1.x: ship what exists

- [x] GitHub release job: a `v*` tag checks the version, lints, runs unit tests, builds,
      and publishes the exe, symbols zip and SHA256SUMS (.github/workflows/ci.yml)
- [x] CI on push to main and on PRs: the same fmt/clippy/unit-test/build steps
- [x] Save the session on Windows sign-out/restart (`WM_QUERYENDSESSION`/`WM_ENDSESSION`)
- [x] Checkpoint the session every 30 seconds, so a crash or kill keeps the tabs
- [ ] Finish the manual checks: cold-cache startup, physical high-DPI/monitors, SMB and clipboard
- [x] Tag a release through the job: v1.2.0, 2026-10-15

### 1.2: everyday-use gaps versus Explorer

- [x] Status bar: selection count and file size, free space on the current drive
- [x] Ctrl+Z / Ctrl+Y undo and redo of file operations, including Recycle Bin restore
- [x] Alt+Enter and a context-menu item open Properties through the shell helper.
      The helper was missing OLE initialization, so Properties from the full menu failed too
- [x] Four thumbnail sizes in View, Ctrl+mouse wheel zoom, DPI-scaled thumbnails
- [x] Date created and Attributes columns, shown or hidden from the header's right-click menu
- [x] Settings/theme stall profiled: identical before and after this work, mostly waiting on
      other windows' threads under synthetic load (HANDOFF item 5)
- Dropped: type-to-select feedback and focus outlines. Windows already provides both; no defect found
- Dropped: dark context menus. No documented API exists; the existing decision to keep native
  light menus stands

### 1.3: distribution and trust

- [x] Code signing step in the release job, active once `SIGNING_PFX`/`SIGNING_PASSWORD` secrets exist
- [ ] Buy or set up a signing certificate (or Azure Trusted Signing): an owner decision
- [ ] winget manifest pointing at the release assets, after the first published release
- [x] Optional "Open in Kestrel" on folder and drive menus, per user, from Settings > Startup
- [x] Help > Check for updates opens the releases page; nothing is sent until the user asks
- [x] Portable mode: a `portable` file beside kestrel.exe keeps data in KestrelData there

### 2.0: power features

- [x] View > Calculate folder sizes: a cancellable worker fills the Size column (Esc stops it).
      Sorting by size still treats folders as empty
- [x] `size:`, `modified:` conditions in the filter and search; `content:word` in the search
      (whole-file scan up to 16 MB per file; one word, no phrases)
- [x] Batch rename (F2 on several items): find/replace, `#` numbering, live preview, one undo
- [x] View > Sync pane navigation (into subfolders and up) and View > Compare panes (selects differences)
- [x] .zip files open as folders through the shell; copy out works. Writing into a zip depends on
      what Windows' zip folder supports
- [x] Custom key bindings in keys.txt, listed first in the F1 guide (read at startup)
- [x] Accessibility: names for the address, filter, file lists, sidebar and tab strip;
      Windows high-contrast colors when contrast mode is on
- [ ] Later: phrase and indexed content search, other archive formats, a key-binding editor in Settings,
      a screen-reader walkthrough with Narrator/NVDA (names were checked through MSAA only)

### Housekeeping

- [x] PLAN.md removed; its completed checklist stays in git history
- [x] Measurements kept here only; HANDOFF.md now holds validation, release steps and limits

Plugins and cloud-drive integrations remain out of scope.
