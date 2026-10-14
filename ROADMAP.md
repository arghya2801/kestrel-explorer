# Kestrel roadmap

[PLAN.md](PLAN.md) contains the implementation checklist.
[HANDOFF.md](HANDOFF.md) records validation commands and remaining release checks.

## Built through 1.1.0

| Area | Implemented |
|---|---|
| Files | Natural sorting, virtual lists, live updates, file operations, clipboard and drag/drop |
| Navigation | History, address editing, breadcrumbs, filter/search, tabs, independent folder panes and windows |
| Sidebar | Quick access pins, drives, Network, active-folder synchronization and child paging |
| View | Details, bounded thumbnail grid, light/dark themes, resizable text/image/shell previews |
| Controls | Toolbar with overflow, Settings pages, shortcut guide, recent folders and terminal launch |
| Persistence | Validated atomic preferences, normal-exit tab/window sessions and missing-folder fallback |
| Diagnostics | Operation status, crash logs/dumps, watchdog, startup probe and expanded stress runner |

1.1.0 version metadata is set locally. Release tagging awaits the manual checks in HANDOFF.

## Current measurements, 2026-10-11

| Measurement | Result |
|---|---|
| Quiet release first paint, seven launches | 102, 99, 114, 98, 134, 95, 91 ms; median 99 ms |
| Release executable | 1,089,024 bytes |
| Working set at first paint | 15,568 to 15,668 KiB; not steady-state memory |
| Restore 50,001 selected names in 100,001 items | 26.44 ms in a release logic test |
| Last-visible icon behind 250 executable requests | 7.52 s before viewport cancellation; 11.56 ms after |
| Applicable regression suite | 131 unit tests and 43 window tests passed |
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

Review of 2026-10-14. Clippy and formatting pass; no application panics outside tests.
Items are ordered by priority inside each phase. Discuss before starting a phase.

### 1.1.x: ship what exists

- [x] GitHub release job: a `v*` tag checks the version, lints, runs unit tests, builds,
      and publishes the exe, symbols zip and SHA256SUMS (.github/workflows/release.yml)
- [ ] CI on push/PR: the same fmt/clippy/unit-test steps, so main stays releasable
- [ ] Save the session on Windows sign-out/restart. `WM_QUERYENDSESSION`/`WM_ENDSESSION` are not
      handled, so an update reboot or log-off loses tabs and the last folder
- [ ] Save the session periodically or on change, so a crash or kill keeps the tabs
- [ ] Finish the manual checks: cold-cache startup, physical high-DPI/monitors, SMB and clipboard
- [ ] Tag v1.1.0 through the release job

### 1.2: everyday-use gaps versus Explorer

- [ ] Status bar: selection count and total size, free space on the current drive
- [ ] Ctrl+Z / Ctrl+Y undo and redo of the last file operation (the shell already records undo)
- [ ] Alt+Enter opens Properties without going through the full menu
- [ ] Ctrl+mouse wheel and View options for icon/thumbnail sizes
- [ ] Additional details columns (created date, attributes) chosen from the header menu
- [ ] Type-to-select feedback and visible focus outline in both themes
- [ ] Dark context menus and dropdowns, if a documented API allows it; light menus remain the fallback
- [ ] Profile the 1.5-second Settings/theme switch seen under stress

### 1.3: distribution and trust

- [ ] Code signing in the release job, so SmartScreen stops warning on download
- [ ] winget manifest (and optionally Scoop) pointing at the release assets
- [ ] Optional "Open folders with Kestrel" registration, reversible from Settings
- [ ] Update check against GitHub releases: notify only, never auto-install
- [ ] Portable mode: settings beside the executable when a marker file exists

### 2.0: power features

- [ ] Folder size calculation on demand (cancellable worker, like search)
- [ ] Search by content and by size/date filters; reuse the search worker
- [ ] Batch rename with preview
- [ ] Synchronized navigation and folder compare between split panes
- [ ] Archive browsing (zip first, through the shell namespace)
- [ ] Custom key bindings, sharing the shortcut table the guide already uses
- [ ] Accessibility pass: screen-reader names for custom-drawn controls, high-contrast theme

### Housekeeping

- [ ] Fold PLAN.md into git history; its checklist is complete. Keep HANDOFF.md for release checks only
- [ ] Measurements appear in README, ROADMAP and HANDOFF; keep one copy here

Plugins and cloud-drive integrations remain out of scope.
