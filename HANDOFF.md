# Kestrel handoff

Updated 2026-11-05. Version 1.4.0; see CHANGELOG.md.
[ROADMAP.md](ROADMAP.md) holds what is built, what remains and the measurements.

## Issue fixes in 1.3.2 awaiting manual checks

- #22: MSI setup now shows destination, progress and completion dialogs. WiX 6.0.2 with the UI extension builds successfully; the compiled MSI's dialogs and navigation were checked. The office machine's failure has not been reproduced.
- #23: periodic checkpoints run off the UI thread, with at most one writer per window. Settings/session disk I/O uses a separate lock from the window state. A blocked-storage regression checks that UI timer messages return promptly and do not queue extra writers. Other causes of the office machine's hangs remain unconfirmed.
- #24: tabs always include their parent, including a drive root. Existing deeper-name disambiguation and full-path tooltips remain.
- #25: This PC and the sidebar enumerate shell devices without drive letters and refresh on device-tree changes. Opening a phone uses Windows' device browser. Native MTP browsing and file operations inside Kestrel are not implemented; a physical phone still needs verification.

## Validation

Latest applicable run, 2026-11-02: **165 unit tests, 63 window tests and the key-binding test passed**,
including the opt-in SMB tests and all three clipboard tests. The original clipboard image was
restored afterward and its image hash verified. Clippy passed across all targets with warnings
denied; formatting and the release build of the binary and examples passed.
WiX 6.0.2 built the MSI; the extracted executable matches the release binary. Compiled installer
tables include destination, progress and completion dialogs, with Welcome leading to destination.
After fixing the icon-font leak found during stress, the standard suite passed again with 163 unit
tests, 60 window tests and the key-binding test. SMB and clipboard checks passed separately earlier
in the session; the final rerun left the changed desktop clipboard untouched.
Window teardown now deletes the cached toolbar icon font. A regression checks that the font handle
is released and the cache cleared across two window lifecycles. The final 120-second release stress
run exited normally after 1,620 UI actions and 41,389 file changes, with a longest UI interval of
406.8 ms. Normalized resource changes were handles +9, GDI +3 and USER +1; GDI had grown by 37 in
the preceding run before the font cleanup. Measurements are in ROADMAP.md.

Run shell/profile/Recycle Bin checks outside the Codex filesystem sandbox. Inside it, batch rename,
Recycle Bin restore and reads of profile ancestors fail with Access denied. These same regressions
pass outside it; the undo test now identifies the operation when it fails.

Tests cover independent panes and tab switching, session round trips and sign-out saving, missing
startup folders, dead-share cancellation, stale work, narrow layouts, sidebar paging past 5,000 children,
large selection restoration, thumbnail resources and sizes, Settings validation/Cancel, guide opening,
pins/breadcrumbs, unpinning standard Quick access folders, This PC, same-name tab labels, vertical tabs,
copy/move/swap/mirror between panes, operation history, undo of every recorded operation (including
Recycle Bin restore), optional columns, and closing a window during a verified 8 MiB copy.

Exclusions:
- One unit and one window SMB test remain opt-in. Both passed against localhost C$ outside the sandbox.
- The routine commands below skip one unit and two window clipboard-writing tests to preserve the
  desktop clipboard. They passed separately with the original clipboard image restored and verified.
  Clipboard-manager contention can still interfere.
- The folder-menu registry test is ignored because it writes HKCU; run it with
  `cargo test --lib folder_menu -- --ignored`. It passed in the previous validation run;
  it was not rerun on 2026-11-02.
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
cargo test --offline --lib network::tests::this_computers_disk_shares_are_listed -- --exact --ignored
cargo test --offline --test window a_computer_lists_its_shares_and_up_returns_to_them -- --exact --ignored
~~~

Do not use the real user data directory for regression tests; the sign-out window test refuses to.

## Releasing

.github/workflows/ci.yml lints, unit-tests and builds every push to main and every PR.
Pushing a tag `vX.Y.Z` that matches Cargo.toml also builds a draft GitHub release with the exe,
the MSI installer (installer/kestrel.wxs, WiX 6), a symbols zip (for crash dumps) and SHA256SUMS.
The MSI is also built on every push. Window tests need a desktop, so run them locally before tagging.
Set the `SIGNING_PFX` (base64) and `SIGNING_PASSWORD` secrets to sign the exe and the MSI.

The MSI is per-user, in %LOCALAPPDATA%\Programs\Kestrel. Never change its UpgradeCode. To test an
install without touching a real one, pass `INSTALLFOLDER=<folder>` to msiexec; the folder is remembered
for upgrade and uninstall. Before that was remembered, an uninstall removed the default folder's
kestrel.exe, so keep a copy of a real install when testing installer changes.

## Remaining release checks and known limits

1. Measure a controlled cold-cache launch. The first sample in a warm series is not a cold-cache result.
2. Exercise physical high-DPI displays and changed monitor arrangements. DPI-scaled fonts, controls,
   thumbnails and window-bound clamping are implemented; physical-device verification remains.
3. Connect a physical phone and verify discovery, reconnect and opening in Windows. No WPD device
   was connected during the 2026-11-02 checks. Re-test MSI setup on the affected office machine.
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
clears modifiers for refresh so Shift cannot trigger a pane copy, bounds its shutdown wait, and
normalizes the layout before comparing resource counts. Its action count varies with machine load
by 2x between sessions: compare builds by alternating runs, not against older logs.
