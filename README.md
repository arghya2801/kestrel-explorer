<p align="center"><img src="docs/icon.png" width="128" alt="Kestrel icon: a yellow folder with a gliding kestrel"></p>

<h1 align="center">Kestrel</h1>

<p align="center">A Windows file explorer built with Rust and Win32.<br>
One executable, no separate runtime.</p>

Kestrel keeps folder listing, search, file operations and icon loading on background workers.
Browse independent folders side by side, switch between tabs, and use Windows' own file operations
and shell integrations. See [ROADMAP.md](ROADMAP.md) for measured startup, size and stress results and [CHANGELOG.md](CHANGELOG.md) for changes.

## Features

- Natural sorting, live folder updates, current-folder filtering and recursive search
- Two independent folder panes in one window, tabs, and additional windows
- Breadcrumb navigation, editable addresses, Quick access pins, recent folders, drives and Network
- A command toolbar below the address bar, with overflow commands under More
- Details and thumbnail grid views in four sizes, hidden-item toggle, adjustable sidebar and preview widths
- Group by date (Today, Yesterday, Last week…), remembered per folder
- Text, image and Windows preview-handler previews
- Copy, cut, paste, rename, batch rename, recycle, permanent delete, new folder, drag/drop and undo/redo
- .zip files open as folders through Windows' built-in zip support; copy files out as from any folder
- Windows' full shell context menu through a separate helper process
- Settings pages, light/dark/system themes, and an in-app keyboard shortcut guide
- Session restoration for tabs, folder panes and window layouts, also after a crash or sign-out
- Status bar with selection count and size and the drive's free space
- Folder sizes on demand, size/date/content search conditions, synced and compared split panes
- Custom key bindings, portable mode, an optional "Open in Kestrel" folder-menu entry
- Screen-reader names and Windows high-contrast colors
- An operation-status window and Open terminal here

## Using the new controls

Choose **Split** or press **Ctrl+Shift+S** for two folder panes. **F6** focuses the other pane.
Each pane keeps its folder, history, selection, sorting and filter. The shared address bar, sidebar,
toolbar and preview follow the active pane. Each tab remembers its panes; drag the center divider
to change their widths. Turning Split off preserves the second pane for reopening.
With **View > Sync pane navigation** on, entering a subfolder or going up in one pane does the same in
the other. **View > Compare panes** selects the files each pane lacks or has with another size or date.
The active pane's path label is tinted with your accent color. Right-click a folder (in the list or the
sidebar) and choose **Open in other pane** to show it there, splitting the view if needed. With two panes,
**Copy to other pane** (**Shift+F5**) and **Move to other pane** (**Shift+F6**) send the selection to the
other pane's folder. **View > Swap panes** (**Ctrl+U**) trades their sides, and **View > Show this folder
in the other pane** (**Ctrl+Shift+U**) opens the active folder in both.

Tabs on different folders with the same name add the parent folder that tells them apart
("MFA221 · bin", "MFA221 · target"); hover a tab for its full path. **View > Tabs above the sidebar**, also
on a tab's right-click menu, lists the tabs in a column above the sidebar instead of along the top.

**Quick access** starts with your Desktop, Downloads, Documents, Pictures, Music and Videos. Right-click a
folder in the list or the sidebar to **Pin to Quick access** or **Unpin from Quick access**; the standard
folders can be unpinned too. Hover a sidebar item for its full path. Click **This PC** in the sidebar
(or type it in the address bar) for a page of drives with their free space and size; **Up** from a drive's
root goes there.

**View** offers Details, four thumbnail sizes, sorting, hidden items and preview visibility.
Each folder remembers the sort you choose in it (a column header or **View**); folders you haven't
sorted use the default sort from **Settings > File list**. So Downloads can stay newest-first while
everything else is alphabetical.
**View > Group by date** sorts the folder into Explorer's date groups (Today, Yesterday, Earlier this
week, Last week…), keeping the chosen sort inside each group; each folder remembers it, so Downloads
can stay grouped while other folders don't. **View > Show in sidebar** (**Ctrl+Shift+E**) expands the
sidebar to the open folder; it doesn't follow navigation on its own.
Right-click the column headers to show Date created or Attributes, or hide other columns.
**More** includes Pin to Quick access, recent folders, parent folders, Open terminal here and File operations.
The filter (**Ctrl+F**) matches names and also understands `size:>10MB`, `size:<1KB`, `modified:<7d`
(changed within a week) and `modified:>2w`. Press **Enter** to search subfolders; there,
`content:word` also finds files containing the word. **View > Calculate folder sizes** fills in folder sizes.
Click a breadcrumb to visit an ancestor; use **Edit** or **Ctrl+L** to type a path.
The terminal command opens Windows PowerShell in the active folder.
File operations shows running jobs and recent results for this process; Windows supplies progress
and cancellation dialogs.

**Settings** has four pages:

| Page | Controls |
|---|---|
| Appearance | Theme, sidebar width, preview visibility/width, split layout and pane ratio |
| File list | Details/grid, hidden items, the default sort and the four main column widths |
| Startup | Startup folder, session restoration and an optional "Open in Kestrel" folder-menu entry |
| Window | Saved position, size and maximized state |

Apply saves changes, OK applies and closes, and Cancel discards edits made since the last Apply.
Restore defaults resets preferences. Theme and pins apply across open windows; view and layout
changes apply to the current view/window. Unsupported preferences such as font size and plugins are not offered;
key bindings live in keys.txt (below).

Settings and logs live in %LOCALAPPDATA%\Kestrel. For portable use, put an empty file named `portable`
beside kestrel.exe; data then goes to a KestrelData folder there. Settings from the old FileExplorerPro
directory migrate once. The session is saved every 30 seconds, at sign-out or restart, and on exit,
so tabs and windows come back after a crash too. Secondary windows still open when the
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
| F2 | Rename; several selected items open batch rename (find/replace, `#` numbers) |
| Del / Shift+Del | Recycle / delete permanently |
| Ctrl+C / Ctrl+X / Ctrl+V | Copy / cut / paste |
| Ctrl+Z / Ctrl+Y | Undo / redo the last file operation |
| Ctrl+Shift+N / Ctrl+Shift+C | New folder / copy paths |
| Ctrl+A | Select all |
| Ctrl+T / Ctrl+W | New tab / close tab |
| Ctrl+Shift+K | Duplicate the active tab |
| Middle-click a tab | Close that tab |
| Ctrl+Tab / Ctrl+Shift+Tab | Next / previous tab |
| Ctrl+N | New window, side by side |
| Ctrl+Shift+S / F6 | Toggle split / focus the other pane |
| Shift+F5 / Shift+F6 | Copy / move the selection to the other pane |
| Ctrl+U / Ctrl+Shift+U | Swap panes / show this folder in the other pane |
| Alt+P | Toggle preview |
| Ctrl+Shift+E | Show the open folder in the sidebar |
| Ctrl+mouse wheel | Thumbnail size; past the smallest, back to Details |
| Shift+mouse wheel | Scroll the file list sideways |
| Tab / Shift+Tab | Next / previous control |
| Shift+F10, Menu key | Context menu |
| Shift+right-click | Full Windows context menu |
| Alt+Enter | Properties |

Text boxes keep their normal editing shortcuts.

### Your own keys

Put a `keys.txt` beside settings.txt (in %LOCALAPPDATA%\Kestrel) with one binding per line, then restart:

~~~text
# keys = command
Ctrl+E = show-in-explorer
Ctrl+Shift+P = properties
F9 = compare-panes
~~~

Your bindings win over the built-in keys and appear at the top of the F1 guide. Keys combine Ctrl, Shift
and Alt with a letter, digit, F1-F24, Left, Right, Up, Down, Home, End, PageUp, PageDown, Insert, Delete,
Backspace, Enter, Space, Tab or Esc. In the address and filter boxes, keys without Ctrl or Alt keep typing.
Commands: open, cut, copy, paste, rename, delete, delete-permanently, new-folder, copy-path,
show-in-explorer, select-all, windows-menu, open-in-new-tab, properties, undo, redo, refresh, hidden-items,
preview, split, settings, shortcuts, details, thumbnails, pin, pin-selected, terminal, file-operations,
folder-sizes, group-by-date, reveal-in-sidebar, sync-panes, compare-panes, open-in-other-pane,
copy-to-other-pane, move-to-other-pane, swap-panes, mirror-pane, vertical-tabs and check-for-updates.
Problems with the file are written to log.txt.

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
| src/listing.rs, sort.rs, groups.rs, search.rs | Reading folders, sorting, date groups and searching |
| src/browser.rs | Navigation and history |
| src/ops.rs, clip.rs, dnd.rs, activity.rs | File operations, clipboard, drag/drop and operation status |
| src/settings.rs, session.rs | Preferences and session persistence |
| src/watch.rs | Folder change notifications |
| src/ui.rs, src/ui/ | Window, panes, tabs, sidebar, toolbar, dialogs, thumbnails and previews |
| res/make_icon.py | Application icon |

