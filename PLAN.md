# Kestrel implementation plan

Updated 2026-10-11. This consolidates HANDOFF.md, the project review, and the requested UI additions.
It is a plan, not a record of implemented features. Unchecked items remain pending.

## Agreed direction

- Add two side-by-side folder views inside one window, with independent navigation.
- Keep the existing window-level tabs. Pane-specific tab bars, synchronized navigation, and folder comparison are outside this pass.
- Add a command toolbar below the address bar.
- Add a Settings dialog for everything already configurable in the application.
- Add an in-app keyboard shortcut guide.
- Keep the Rust and Win32 implementation, background workers, existing Windows file operations, and shell-extension isolation.

## Current baseline

The existing application has listing, navigation, sidebar, live updates, file operations, clipboard integration,
drag and drop, shell menus, filter/search, tabs, dark mode, previews, side-by-side windows, and network shares.
Side-by-side windows are implemented; two folder panes in one window are pending.
Preview thumbnails exist; a thumbnail grid for the file list is pending.

The 2026-10-11 review found a clean working tree at 97e1f59. Formatting and Clippy passed.
There are 124 unit test declarations and 32 window tests. Of 123 unit tests run, 122 passed and the
localhost-share test failed with Access is denied. The real-clipboard test was skipped.
Window tests, release startup timings, and the 10-minute stress test were not rerun.
These results describe that review, not a guarantee for later changes.

## 1. Restore and verify the performance baseline

- [ ] Measure warm and cold release startup on the same machine and record each existing startup stage.
- [ ] Investigate the recorded first-paint regression from 54-70 ms to 141-196 ms.
- [ ] Create hidden preview controls on first use if measurements show a worthwhile improvement.
- [ ] Measure the cost of applying dark mode before first paint; preserve readable controls and avoid a visible theme flash.
- [ ] Investigate DLL loading only after measurements identify its contribution. Do not add delay-loading machinery without evidence.
- [x] Run the existing 600-second release stress test, recording handle, GDI, USER counts, and worst stall.
- [ ] Keep the historical 5.4-second stall open until there is evidence explaining it. Inspect the watchdog dump if it recurs.
- [x] Separate environment-dependent test prerequisites from application failures. The localhost-share test currently assumes C$ is available.
- [x] Document clipboard contention and focus-sensitive test limitations.

Pass condition: measured startup improves, repeated stress runs show no sustained resource growth, and remaining failures have recorded causes.
Update the measurements before the proposed v1.1.0 stabilization release. Update Cargo, lockfile metadata as needed, and Windows resource versions together.

Touches: src/main.rs, src/ui.rs, src/ui/preview.rs, src/ui/dark.rs, build.rs only if justified,
examples/stress.rs, affected unit/window tests, Cargo.toml, Cargo.lock, res/app.rc, README.md, ROADMAP.md, HANDOFF.md.

## 2. Add the toolbar, Settings, and shortcut guide

### Toolbar

- [x] Keep Back, Forward, Up, address, and filter on the top row.
- [x] Put a command row immediately below it, with New folder, Cut, Copy, Paste, Rename, Delete, Refresh, and dropdowns for View and More.
- [x] Put Split, Settings, and Help on the command row. Split becomes available with step 3.
- [x] View exposes sorting, hidden items, and preview visibility; additional view modes appear only when implemented.
- [x] More exposes Copy as path, Select all, Show in Explorer, and the existing full Windows menu.
- [x] Disable selection-dependent actions when unavailable. Give every button a readable label or tooltip and keyboard access.
- [x] At narrow widths, move lower-priority commands into More rather than clipping the address bar or buttons.
- [x] Route toolbar, menus, and keyboard actions through the existing command handling. Preserve text-box editing behavior and restore list focus appropriately.

### Settings

Expose existing settings first, grouped into these pages:

| Page | Existing configurable values |
|---|---|
| Appearance and layout | System/light/dark theme, sidebar width, preview visibility |
| File list | Show hidden items, sort column and direction, widths of Name/Size/Modified/Type columns |
| Startup | The saved folder used on the next launch; an explicit command-line folder continues to win |
| Window | Saved position and size, maximized state, and a reset of window bounds |

- [x] Add Apply, OK, Cancel, and Restore defaults for preferences. Resetting preferences must not delete files.
- [x] Validate folder input and numeric sizes; unavailable folders follow the existing fallback behavior.
- [x] Preserve existing settings files, the old-name migration, atomic saves, and defaults for missing or malformed keys.
- [ ] Apply live changes to open windows where appropriate; save through one owner so windows do not overwrite each other's settings.
- [ ] Distinguish global defaults from current view/session state so changing a default does not unexpectedly reset another window.
- [x] Add controls for new split/preview dimensions when those features are implemented.

Font size, density, click behavior, custom key bindings, plugins, and other currently unsupported preferences are not part of this Settings pass.

### Shortcut guide

- [x] Open it through Help > Keyboard shortcuts and F1.
- [x] Group shortcuts into Navigation, Files, Search, Tabs/windows, and View.
- [x] Show action, keys, and scope, including cases where a key keeps its normal meaning in an edit box.
- [x] Include every supported shortcut, including Ctrl+Shift+Tab, Shift+F10, Shift+Del, and the new split-pane commands when added.
- [ ] Keep the guide consistent with actual command handling and menu labels. Once shared shortcut definitions exist, use them for the guide.
- [ ] Make the guide keyboard navigable, readable in both themes, and usable at high DPI.

Pass condition: common operations work from toolbar, menus, and keyboard; Settings changes survive restart;
Cancel does not apply edits; Help opens a guide that matches the shortcuts.

Touches: src/ui.rs, src/ui/commands.rs, src/ui/dark.rs, src/settings.rs, src/main.rs,
small UI modules for the toolbar/settings/help as needed, tests/window.rs, relevant settings tests, README.md.

## 3. Add two-pane navigation inside one window

- [x] Toggle between one folder view and two side-by-side folder views.
- [x] Give each pane its own folder, history, selection, scroll position, sorting, filter/search, loading cancellation, and watcher.
- [x] Use a draggable center divider and a clear active-pane indication.
- [x] Keep one address/filter row, sidebar, command toolbar, and preview area; they follow the active pane.
- [x] Keep the existing single tab strip. Each tab remembers its one- or two-pane layout and each pane's browsing state.
- [x] New split starts with the second pane at the current folder. Turning split off preserves the second pane's state for reopening.
- [x] Route navigation, file commands, menus, and drops to the intended pane.
- [x] Capture operation paths and destinations when commands start, so later focus changes cannot redirect a copy, move, or rename.
- [x] Identify asynchronous results by pane and tab as well as generation. Late results must not update a different pane.
- [x] Preserve in-progress operations and discard stale UI results when a pane, tab, or window closes.
- [x] Add documented commands for toggling split and focusing the other pane; choose keys after checking current shortcuts.

Implementation approach: move the state that actually belongs to a folder view out of the current single-view App fields.
Reuse Browser, Tabs, listing/sorting, Events, icons, Watcher, ops, clipboard, and drag/drop behavior.
Update their callers together; avoid a second copy of the same navigation and operation logic.

Pass condition: both panes browse independently, an unreachable share in one leaves the other responsive,
selection and operations stay in the correct pane, and tab switches restore both views correctly.

Touches: src/ui.rs, src/ui/tabbar.rs, src/ui/commands.rs, src/ui/tree.rs, src/ui/drag.rs,
src/ui/preview.rs, src/tabs.rs, src/browser.rs only where navigation requires it, src/settings.rs,
event callers and routing, tests/window.rs, relevant logic tests.

## 4. Finish navigation, persistence, and layout gaps

- [ ] Sidebar follows the active pane's folder.
- [ ] Hidden-item changes invalidate and reload affected sidebar children.
- [ ] Add a Network node. Network enumeration must run on workers; typed server paths remain usable when discovery is unavailable.
- [ ] Restore open tabs, active tab, split layout, and each pane's folder.
- [ ] Save and restore secondary-window state through one coordinated writer; retain every window's state instead of letting the last writer win.
- [ ] Restore missing folders gracefully and fit window bounds to the current monitors.
- [x] Give the preview pane a draggable divider and persist its width.
- [x] Fix repeated Back presses during a load; check the same situation for Forward and cancellation.

Pass condition: restart restores the session, secondary windows retain their settings, sidebar state matches the active folder,
and fast navigation or hidden-item changes do not apply stale results.

Touches: src/browser.rs, src/ui.rs, src/ui/tree.rs, src/ui/tabbar.rs, src/ui/preview.rs,
src/settings.rs, src/main.rs, navigation/settings tests, tests/window.rs.

## 5. Remove large-folder limits and add the missing view

- [ ] Measure selection restoration in 100k+ item folders and replace avoidable per-row work while preserving selection and focus.
- [ ] Replace the silent 5,000-child sidebar cap with incremental loading or a visible Load more action.
- [ ] Measure icon lag while scrolling executable-heavy folders; discard requests for rows no longer needed if lag is confirmed.
- [ ] Add large-icon/thumbnail grid view as an explicit later feature, selectable through View and Settings.
- [ ] Request thumbnails only for needed rows, load them off the UI thread, bound retained image resources, and discard stale results.

Pass condition: large folders remain responsive, all sidebar children can be reached, and repeated view changes do not leak image resources.

Touches: src/ui.rs, src/ui/tree.rs, src/icons.rs, src/listing.rs where needed,
thumbnail/preview helpers, src/settings.rs, relevant logic tests, tests/window.rs, examples/stress.rs.

## 6. Useful additions after the requested UI

These are proposed additions, not prerequisites for the first toolbar/settings release.

- [ ] Pin and unpin folders in Quick access; persist the user's pins.
- [ ] Add a breadcrumb address display with ancestor navigation while preserving Ctrl+L for editable paths.
- [ ] Add a recent-folders menu, excluding passwords or credentials from saved locations.
- [ ] Add an operation-status view showing running jobs, destinations, completion, and errors, using existing Windows progress behavior.
- [ ] Add Open terminal here to More, using the active pane's folder and reporting launch failures.

For each addition, define its saved state and affected commands before implementation.
Use existing workers and Windows integrations rather than adding a dependency for a small feature.

## 7. Release checks for each milestone

- [ ] Run formatting, Clippy across all targets, unit tests, and window tests appropriate to the change.
- [ ] Report environment-dependent exclusions explicitly; do not report a partially run suite as fully green.
- [ ] Expand the stress harness to include filter/search, tab switches, split-pane navigation, preview toggles,
      settings/theme changes, and secondary-window creation/closure.
- [ ] Run the expanded 600-second release stress test and record resource counts and worst stall.
- [ ] Check startup, executable size, and memory after UI additions. Compare measurements taken under similar conditions.
- [ ] Exercise dead network shares, large folders, rapid refresh/navigation, unavailable startup locations,
      and closing windows during copies.
- [ ] Check keyboard-only use, focus order, narrow windows, high DPI, changed monitor arrangements, and both themes.
- [ ] Update README, ROADMAP, HANDOFF, and this checklist to match what actually shipped.
- [ ] Update package/resource versions together and tag only a verified milestone.

## Deliberately retained limits

- Light native menus remain acceptable until dark menus can be implemented without undocumented uxtheme ordinals.
- Shortcut-arrow overlays stay deferred unless safe worker-based loading has a measured acceptable cost.
- Plugins, cloud integrations, archive browsing, and an in-process Properties dialog remain outside this plan.
- No release date is assumed. Stabilization comes first, followed by toolbar/settings/help, two-pane navigation, and the remaining polish.

## Implementation results

2026-10-11: toolbar, Settings pages, and F1 shortcut guide added. Clippy passed; 31 window tests passed,
one SMB test was explicitly ignored and two clipboard-writing tests were excluded.
The baseline 600-second stress run completed 11,713 UI actions and 213,177 file changes.
Longest loop stall was 101.29 ms; growth after warm-up was handles -14, GDI 0, USER -1.
Two individual early paint messages took 128 ms and 166 ms. A debug build ran during this test;
repeat under quiet conditions after the remaining features. Startup first-paint measurements remain pending.

2026-10-11: independent folder panes, pane-scoped worker results and operations, per-tab view controls,
center/preview dividers, and Ctrl+Shift+S / F6 added. The window regression run passed 33 tests
before exposing a filter casing regression; that regression was fixed and its test passed separately.
Clippy passed. SMB remains ignored; two clipboard-writing tests were excluded.
