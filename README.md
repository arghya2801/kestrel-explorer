<p align="center"><img src="docs/icon.png" width="128" alt="Kestrel icon: a yellow folder with a gliding kestrel"></p>

<h1 align="center">Kestrel</h1>

<p align="center">A Windows file explorer built with Rust and Win32.<br>
One executable, no separate runtime.</p>

Kestrel keeps folder listing, search, file operations and icon loading on background workers.
Browse independent folders side by side, switch between tabs, and use Windows' own file operations
and shell integrations. See [ROADMAP.md](ROADMAP.md) for measured startup, size and stress results.

## Features

- Natural sorting, live folder updates, current-folder filtering and recursive search
- Two independent folder panes in one window, tabs, and additional windows
- Breadcrumb navigation, editable addresses, Quick access pins, recent folders, drives and Network
- A command toolbar below the address bar, with overflow commands under More
- Details and thumbnail grid views in four sizes, hidden-item toggle, adjustable sidebar and preview widths
- Text, image and Windows preview-handler previews
- Copy, cut, paste, rename, recycle, permanent delete, new folder and drag/drop
- Windows' full shell context menu through a separate helper process
- Settings pages, light/dark/system themes, and an in-app keyboard shortcut guide
- Session restoration for tabs, folder panes and window layouts
- An operation-status window and Open terminal here

## Using the new controls

Choose **Split** or press **Ctrl+Shift+S** for two folder panes. **F6** focuses the other pane.
Each pane keeps its folder, history, selection, sorting and filter. The shared address bar, sidebar,
toolbar and preview follow the active pane. Each tab remembers its panes; drag the center divider
to change their widths. Turning Split off preserves the second pane for reopening.

**View** offers Details, four thumbnail sizes, sorting, hidden items and preview visibility.
Right-click the column headers to show Date created or Attributes, or hide other columns.
**More** includes pin/unpin, recent folders, parent folders, Open terminal here and File operations.
Click a breadcrumb to visit an ancestor; use **Edit** or **Ctrl+L** to type a path.
The terminal command opens Windows PowerShell in the active folder.
File operations shows running jobs and recent results for this process; Windows supplies progress
and cancellation dialogs.

**Settings** has four pages:

| Page | Controls |
|---|---|
| Appearance | Theme, sidebar width, preview visibility/width, split layout and pane ratio |
| File list | Details/grid, hidden items, sort column/direction and the four main column widths |
| Startup | Startup folder and session restoration |
| Window | Saved position, size and maximized state |

Apply saves changes, OK applies and closes, and Cancel discards edits made since the last Apply.
Restore defaults resets preferences. Theme and pins apply across open windows; view and layout
changes apply to the current view/window. Unsupported preferences such as custom key bindings,
font size and plugins are not offered.

Settings and logs live in %LOCALAPPDATA%\Kestrel. Settings from the old FileExplorerPro directory
migrate once. Session restoration runs after a normal exit. Secondary windows still open when the
original window closes are included; close a secondary window first to omit it from the next session.
A command-line folder bypasses restoration. Restart restores folders and layout, while navigation
history and selections remain specific to the running session.

## Keyboard

Press **F1**, or choose **Help > Keyboard shortcuts**, for the in-app guide.

| Keys | Action |
|---|---|
| Enter, double-click | Open |
| Backspace, Alt+Left / Alt+Right | Back / Forward |
| Alt+Up | Parent folder |
| Ctrl+L, Alt+D, F4 | Edit address |
| Ctrl+F | Filter; Enter searches subfolders |
| Esc | Cancel loading/search; clear the focused filter or restore the address |
| F5 / Ctrl+H | Refresh / show hidden items |
| F2 | Rename |
| Del / Shift+Del | Recycle / delete permanently |
| Ctrl+C / Ctrl+X / Ctrl+V | Copy / cut / paste |
| Ctrl+Z / Ctrl+Y | Undo / redo the last file operation |
| Ctrl+Shift+N / Ctrl+Shift+C | New folder / copy paths |
| Ctrl+A | Select all |
| Ctrl+T / Ctrl+W | New tab / close tab |
| Ctrl+Tab / Ctrl+Shift+Tab | Next / previous tab |
| Ctrl+N | New window, side by side |
| Ctrl+Shift+S / F6 | Toggle split / focus the other pane |
| Alt+P | Toggle preview |
| Ctrl+mouse wheel | Thumbnail size; past the smallest, back to Details |
| Tab / Shift+Tab | Next / previous control |
| Shift+F10, Menu key | Context menu |
| Shift+right-click | Full Windows context menu |
| Alt+Enter | Properties |

Text boxes keep their normal editing shortcuts.

## Build

Requires Windows 10/11, Rust stable with the MSVC toolchain, and Visual Studio Build Tools.

~~~powershell
cargo build --release
target\release\kestrel.exe [folder]
~~~

Closing the windows leaves the process running until pending copies and moves finish.
Shell context-menu handlers run in a helper process, and preview handlers use Windows' preview host.
Crash logs and watchdog dumps are written under %LOCALAPPDATA%\Kestrel.

## Tests and measurements

~~~powershell
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --check
cargo run --release --example startup -- 7
cargo run --release --example bench -- C:\Windows\System32
cargo run --release --example stress -- 600
~~~

Unit tests cover navigation, listing, settings, sessions, file operations and worker behavior.
Window tests drive real Win32 controls. Clipboard tests write the desktop clipboard, and focus tests
should run while the desktop is idle. SMB administrative-share tests are opt-in and require a
readable localhost C$ share:

~~~powershell
cargo test --lib this_computers_disk_shares_are_listed -- --ignored
cargo test --test window a_computer_lists_its_shares_and_up_returns_to_them -- --ignored
~~~

The ignored executable-icon and visual-capture diagnostics can be run separately.
[HANDOFF.md](HANDOFF.md) records exact checks, exclusions and remaining release work.

## Source layout

| Path | Responsibility |
|---|---|
| src/listing.rs, sort.rs, search.rs | Reading folders, sorting and searching |
| src/browser.rs | Navigation and history |
| src/ops.rs, clip.rs, dnd.rs, activity.rs | File operations, clipboard, drag/drop and operation status |
| src/settings.rs, session.rs | Preferences and session persistence |
| src/watch.rs | Folder change notifications |
| src/ui.rs, src/ui/ | Window, panes, tabs, sidebar, toolbar, dialogs, thumbnails and previews |
| res/make_icon.py | Application icon |

[PLAN.md](PLAN.md) tracks implementation and verification.
