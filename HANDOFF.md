# Kestrel handoff

Updated 2026-10-11. The application additions in [PLAN.md](PLAN.md) are implemented.
Package, lockfile and Windows resources are at 1.1.0. No new release tag or push has been made;
the remaining manual release checks are listed below.

## Implemented in this pass

- Command toolbar below the address row, with narrow-window overflow under More
- Settings for theme, file view, hidden items, sorting, column widths, startup/session behavior,
  window bounds, sidebar, split and preview layout
- F1 / Help keyboard shortcut guide with editing-scope notes
- Independent side-by-side folder panes, Ctrl+Shift+S toggle, F6 focus switch, per-tab pane state
- Coordinated tab/window session persistence, missing-folder fallback and monitor-bound clamping
- Sidebar synchronization, hidden-item invalidation, Network discovery and visible child paging
- Batched large selections and cancellation of obsolete executable-icon requests
- Bounded thumbnail grid workers/cache, stale-result disposal and preview bitmap ownership fixes
- Quick access pins, breadcrumb navigation, recent folders, PowerShell launch and operation status
- Deferred drag-target initialization, startup probe and expanded stress coverage
- A listing queue that preserves restored requests when the eight worker slots are occupied

The active pane owns file commands, history, selection, filter, sort and watcher state.
Worker results carry the view identity and generation; closed or superseded views reject them.
Changing focus after starting a file operation cannot change its captured paths.

## Validation

Final full applicable run: **131 unit tests and 43 window tests passed**.
Clippy passed across all targets with warnings denied; formatting passed.
After the last dark-header drawing change, its window test, Clippy and the release build passed.
Both-theme captures and the actual release dark/grid capture were inspected.

Tests cover independent panes and tab switching, session round trips, missing startup folders,
dead-share cancellation, stale work, narrow layouts, sidebar paging past 5,000 children,
large selection restoration, thumbnail resources, Settings validation/Cancel, guide opening,
pins/breadcrumbs, operation history, and closing a window during a verified 8 MiB copy.

Exclusions:
- One unit and one window SMB test are explicitly ignored because readable localhost C$ is unavailable.
- One unit and two window clipboard-writing tests were skipped to preserve the desktop clipboard.
  Clipboard-manager contention remains an environment limitation.
- The ignored executable-icon diagnostic passed separately. The ignored render diagnostic also passed.
- Focus-sensitive window tests serialize desktop access, but other desktop activity can still interfere.

~~~powershell
$env:LOCALAPPDATA = Join-Path $PWD 'target/final-test-data'
cargo test --offline --lib -- --test-threads=1 --skip files_survive_a_trip_through_the_clipboard
cargo test --offline --test window -- --test-threads=1 --skip copy_and_cut_then_paste_into_another_folder --skip copy_as_path_puts_quoted_paths_on_the_clipboard
cargo clippy --offline --all-targets -- -D warnings
cargo fmt --check
cargo build --offline --release --bins --examples
~~~

Do not use the real user data directory for regression tests.

## Measurements

Final quiet release launch samples were **102, 99, 114, 98, 134, 95 and 91 ms** to first paint,
median **99 ms**. First-paint working set was **15,568 to 15,668 KiB**, roughly 15.2 to 15.3 MiB.
This is memory at first paint, before all background initialization, not steady-state memory.
Executable size was **1,089,024 bytes**.

The immediate pre-optimization instrumented run measured 114 to 126 ms warm.
Drag-target creation/registration accounted for 18 to 23 ms and now follows first paint.
Hidden preview controls cost 2 to 4 ms and theme application 0 to 1 ms, so both were retained.
There is no evidence justifying DLL delay-loading machinery.
These samples are indicative comparisons on the same machine, not a controlled cold-cache study.

Selection restoration for 100,001 items and 50,001 selected names took **26.44 ms** in a release test.
The 250-executable icon probe improved from **7.52 s** behind obsolete requests to **11.56 ms**
with viewport cancellation. Both measurements are diagnostic samples, not timing guarantees.

The quiet expanded 600-second run completed **7,942 UI actions and 210,141 file changes** without
application errors. The worst measured loop/action interval was **1.585 seconds**. Its largest
individually logged action was a **1.560-second Settings/theme cycle**, with 78 ms of UI-thread CPU time.
Resources versus the warm-up layout were **handles -44, GDI +7, USER -5**. The GDI difference prompted
a focused follow-up with a fixed theme and settled thumbnail state.

A 120-second follow-up used the same light theme and settled grid state at each resource sample.
It completed 1,483 actions and 42,405 file changes. Samples of handles/GDI/USER were
(528,80,58), (525,80,58), (528,79,58), and final (523,79,56), giving changes -5/-1/-2.
No sustained resource growth was observed. This supports state variation as the explanation for
the earlier +7 GDI result, rather than a demonstrated leak. Worst interval was 1.542 seconds.
The stress runner now records repeated comparable samples; no application code changed for this follow-up.

Raw local artifacts are under target/startup-final.log, target/stress-final.log, target/stress-resource.log,
target/stress-final-data/Kestrel/log.txt and target/ui-review.

## Remaining release checks and known limits

1. Measure a controlled cold-cache launch. The first sample in a warm series is not a cold-cache result.
2. Exercise physical high-DPI displays and changed monitor arrangements. DPI-scaled fonts/control
   handling and window-bound clamping are implemented; physical-device verification remains.
3. Re-run opt-in SMB and clipboard tests on a suitable desktop before release sign-off.
4. The historical 5.4-second stall remains unexplained. It occurred during the first old stress run
   with concurrent builds/launches. Read the watchdog dump under %LOCALAPPDATA%\Kestrel\crashes if it recurs.
5. Settings/theme cycles reached about 1.56 seconds under continuous stress. Profile them further if
   comparable pauses occur during ordinary use; this is separate from the historical stall.
6. Tag a verified milestone after the manual checks. Version metadata alone is not release sign-off.

Native menus remain light. Shortcut-arrow overlays remain deferred because installed overlay
handlers can be slow. Network discovery depends on Windows services; typed UNC paths remain usable.
Session state saves on normal exit; it does not provide crash recovery or restore navigation history.
Open terminal here was compiled and its working-directory construction reviewed, but no interactive
terminal was launched during verification.

Plugins, cloud integrations, archive browsing, custom key bindings and an in-process Properties
dialog remain outside the agreed scope.

## Working here

Use Rust stable MSVC and PowerShell. Cargo is available in this environment.
Read connected callers and preserve asynchronous error handling when changing the UI.
Use the existing workers and Events queue for blocking work. Keep logic tests alongside plain
modules and use tests/window.rs for control behavior. Commit each verified increment.

Settings/logs use %LOCALAPPDATA%\Kestrel; the repository directory remains file-explorer-pro.
Main owns final session saving after all windows and file operations finish.
Global theme/pin changes propagate to windows; view/layout changes stay local.
The startup probe launches the actual executable with isolated settings and closes only its own children.
The stress runner bounds message-drain batches so synthetic input cannot starve behind worker messages,
and normalizes the layout before comparing resource counts.
