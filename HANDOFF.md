# Kestrel handoff

Updated 2026-10-22. Version 1.3.0 plus Unreleased changes; see CHANGELOG.md.
[ROADMAP.md](ROADMAP.md) holds what is built, what remains and the measurements.

## Validation

Latest full applicable run: **157 unit tests, 60 window tests and the key-binding test passed**.
Clippy passed across all targets with warnings denied; formatting passed.

Tests cover independent panes and tab switching, session round trips and sign-out saving, missing
startup folders, dead-share cancellation, stale work, narrow layouts, sidebar paging past 5,000 children,
large selection restoration, thumbnail resources and sizes, Settings validation/Cancel, guide opening,
pins/breadcrumbs, unpinning standard Quick access folders, This PC, same-name tab labels, vertical tabs,
copy/move/swap/mirror between panes, operation history, undo of every recorded operation (including
Recycle Bin restore), optional columns, and closing a window during a verified 8 MiB copy.

Exclusions:
- One unit and one window SMB test are explicitly ignored because readable localhost C$ is unavailable.
- One unit and two window clipboard-writing tests were skipped to preserve the desktop clipboard.
  Clipboard-manager contention remains an environment limitation.
- The folder-menu registry test is ignored because it writes HKCU; run it with
  `cargo test --lib folder_menu -- --ignored`. It passed and restores the previous state.
- The ignored executable-icon and render diagnostics can be run separately.
- Focus-sensitive window tests serialize desktop access, but other desktop activity can still interfere.

~~~powershell
$env:LOCALAPPDATA = Join-Path $PWD 'target/final-test-data'
cargo test --offline --lib -- --test-threads=1 --skip files_survive_a_trip_through_the_clipboard
cargo test --offline --test window -- --test-threads=1 --skip copy_and_cut_then_paste_into_another_folder --skip copy_as_path_puts_quoted_paths_on_the_clipboard
cargo test --offline --test keys
cargo clippy --offline --all-targets -- -D warnings
cargo fmt --check
cargo build --offline --release --bins --examples
~~~

Do not use the real user data directory for regression tests; the sign-out window test refuses to.

## Releasing

.github/workflows/ci.yml lints, unit-tests and builds every push to main and every PR.
Pushing a tag `vX.Y.Z` that matches Cargo.toml also builds a draft GitHub release with the exe,
a symbols zip (for crash dumps) and SHA256SUMS. Window tests need a desktop, so run them locally
before tagging. Set the `SIGNING_PFX` (base64) and `SIGNING_PASSWORD` secrets to sign releases.

## Remaining release checks and known limits

1. Measure a controlled cold-cache launch. The first sample in a warm series is not a cold-cache result.
2. Exercise physical high-DPI displays and changed monitor arrangements. DPI-scaled fonts, controls,
   thumbnails and window-bound clamping are implemented; physical-device verification remains.
3. Re-run opt-in SMB and clipboard tests on a suitable desktop before release sign-off.
4. The historical 5.4-second stall remains unexplained. Read the watchdog dump under
   %LOCALAPPDATA%\Kestrel\crashes if it recurs.
5. Settings/theme cycles still reach 1.1 to 1.7 seconds under synthetic stress, equally before and after
   the 2026-10-15 changes. Only 31 to 78 ms of that is UI-thread CPU; the rest is waiting, consistent with
   activation messages to other windows' UI threads that the stress load keeps busy.

Native menus remain light. Shortcut-arrow overlays remain deferred because installed overlay
handlers can be slow. Network discovery depends on Windows services; typed UNC paths remain usable.
The session is checkpointed every 30 seconds and at sign-out; navigation history is not restored.
Open terminal here was compiled and its working-directory construction reviewed, but no interactive
terminal was launched during verification.

## Working here

Use Rust stable MSVC and PowerShell. Cargo is available in this environment.
Read connected callers and preserve asynchronous error handling when changing the UI.
Use the existing workers and Events queue for blocking work. Keep logic tests alongside plain
modules and use tests/window.rs for control behavior. Commit each verified increment.

Settings/logs use %LOCALAPPDATA%\Kestrel (or KestrelData beside the exe in portable mode);
the repository directory remains file-explorer-pro.
Main owns final session saving after all windows and file operations finish.
Global theme/pin changes propagate to windows; view/layout changes stay local.
File operations go through ops::run, which records undo; add new operations there.
The startup probe launches the actual executable with isolated settings and closes only its own children.
The stress runner bounds message-drain batches so synthetic input cannot starve behind worker messages,
and normalizes the layout before comparing resource counts. Its action count varies with machine load
by 2x between sessions: compare builds by alternating runs, not against older logs.
