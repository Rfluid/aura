---
title: Platform integration — clickable tray icon
status: draft
version: 0.1.0
last_updated: 2026-05-24
last_verified: 2026-05-24
source_refs:
  - crates/aura/src/tray.rs
  - crates/aura/src/main.rs
  - crates/aura/src/platform.rs
  - crates/aura/src/work_area.rs
owner: "@rfluid"
tags: [architecture, platform, docs]
---

# Platform integration: making Aura a clickable tray icon everywhere

Aura is a tray-indicator app: the icon next to the clock is the *entire* UI.
Primary-click toggles a small modal anchored near the icon; right-click
opens a tiny context menu (Show / Quit). Achieving this UX on Linux, macOS,
and Windows with a single GPUI codebase requires a different combination of
host APIs on each platform. This document is the map.

## Tray icon backend

| Platform        | Crate / API                              | Why                                                                                                   | Source              |
| --------------- | ---------------------------------------- | ----------------------------------------------------------------------------------------------------- | ------------------- |
| Linux / BSD     | [`ksni`](https://crates.io/crates/ksni)  | StatusNotifierItem over D-Bus. KDE/GNOME/Cinnamon emit `Activate()` on primary-click — the one-click UX. | `tray.rs::linux`    |
| macOS / Windows | [`tray-icon`](https://crates.io/crates/tray-icon) | AppKit `NSStatusItem` / Win32 `Shell_NotifyIconW`. Native handlers, no GTK dependency.              | `tray.rs::non_linux` |

The two backends share a `TrayEvent` enum (`Show { hint: Option<(i32, i32)> }`
/ `Quit`) and a `try_recv_event()` poller; the main loop drains events
every 150 ms regardless of platform.

The Linux `ksni` backend uses `libayatana-appindicator`'s wire protocol but
talks D-Bus directly — `tray-icon`'s `gtk` feature refuses to surface
primary-click on AppIndicator hosts (it expects a menu and treats click as
"open menu"), so we picked an SNI-native implementation instead.

## Modal positioning

`main.rs::compute_modal_bounds()` decides where the modal opens; the choice
is OS-specific because that's where the tray icon lives:

- **macOS**: tray icon is at the **top** in the menu bar. Modal anchors
  ~25 pt below the bar, horizontally centred on the click X coordinate
  (`hint.0` from `TrayEvent::Show`).
- **Linux / Windows**: tray icon is in the bottom-right (or wherever the
  user put their panel/taskbar). Modal anchors to the bottom-right of the
  *work area* (display minus reserved panel space — see
  [Work-area detection](#work-area-detection)).
  Wayland compositors may ignore the requested origin and centre the
  window. KDE users can install a window rule matching `app_id="aura"` to
  force the position (`Window matches: WM_CLASS = aura` → Position =
  Apply Initially). See the README "Modal placement on Wayland" section.

## Click-outside auto-dismiss

The main poll loop polls `cx.active_window()` every 150 ms; when it returns
`None` (no GPUI window is the OS-foreground window) the modal is closed.

A grace period of four polls (~600 ms) starts each time the modal opens,
because:

- **Windows**: `cx.activate(true)` is a no-op; Win32 has to deliver focus
  asynchronously after the window is mapped.
- **Wayland (KDE/GNOME/etc.)**: focus is delivered after the surface has
  been mapped and committed; on some compositors that takes a frame or two.

Without the grace period the modal would close itself before the user
saw it.

The hidden keepalive window (next section) is `minimize_window()`'d at
startup and never becomes "active" in the OS sense, so it doesn't poison
the focus check.

## Keepalive window

`main.rs::open_keepalive_window` opens a 1×1, off-screen, minimized GPUI
window at startup. Reason:

> GPUI 0.2's Wayland backend exits the event loop when the last window
> closes (`wayland/client.rs` checks `state.windows.is_empty()`). The tray
> icon is *not* a GPUI window, so the moment the user closes the modal we'd
> lose the process. The keepalive guarantees `state.windows.len() ≥ 1` for
> the lifetime of the tray.

Hardening (per call-site):

- Origin at `(-9999, -9999)` so even if the compositor doesn't clamp it
  back on-screen, the user can't accidentally focus or click it.
- `minimize_window()` immediately — KDE puts it straight into the taskbar
  overflow instead of painting it on the desktop.
- `app_id = "aura-keepalive"` so KDE's task manager doesn't group it under
  the main "Aura" entry.
- `on_window_should_close` returns `false` — clicking the compositor's
  "close window" action on the keepalive becomes a no-op, so the tray
  can't be killed by a stray click. The internal `window.remove_window()`
  used by the toggle path bypasses this guard (it's an internal close,
  not a platform request).

The keepalive is harmless on macOS/Windows where GPUI doesn't quit on
last-window-close, but the unconditional opening keeps the code path the
same on all platforms.

## Single-instance guard

`platform::acquire_single_instance()` (called at the top of `main()`)
prevents two Aura tray icons from racing during autostart. Returns `false`
if another instance already holds the lock; `main()` returns `Ok(())`
silently in that case so the user just sees the existing tray icon.

| Platform | Mechanism                                                 | Lock path                                              |
| -------- | --------------------------------------------------------- | ------------------------------------------------------ |
| Unix     | `flock(fd, LOCK_EX \| LOCK_NB)` on a per-user lockfile.   | `$XDG_RUNTIME_DIR/aura.lock` (tmpfs on systemd); else `$TMPDIR/aura-<uid>.lock`. |
| Windows  | `CreateMutexW(L"Local\\AuraSingleInstance")` + `GetLastError() == ERROR_ALREADY_EXISTS`. | (no file path; kernel object) |

Both implementations leak their handle / fd intentionally — the OS
releases the lock on process exit (including SIGKILL / panic), so a Drop
dance isn't required and can't be sabotaged by a panic during shutdown.

## Work-area detection

The modal's auto-resize callback (`app.rs::on_children_prepainted`) caps
its height so it never grows into a bottom taskbar / Dock. The cap source
ladder lives in `work_area::available_bottom`:

| Source                                                  | Coverage                                                              |
| ------------------------------------------------------- | --------------------------------------------------------------------- |
| `NSScreen.visibleFrame.origin.y` (macOS)                | Subtracts the bottom Dock height (0 when auto-hidden or side-docked). |
| `SystemParametersInfoW(SPI_GETWORKAREA)` (Windows)      | Returns physical work-area rect; we cache the bottom-fraction so DPI scale cancels out. |
| `~/.config/plasmashellrc` + `plasma-org.kde.plasma.desktop-appletsrc` (Linux KDE Plasma) | Reads the thickest bottom-anchored panel's `thickness=` value. KDE's `StrutManager.availableScreenRect` D-Bus API returns the full display rect on the versions we tested — config parsing is the workaround. |
| `xprop -root _NET_WORKAREA` (Linux X11 / XWayland)      | Reads the EWMH work-area property. Covers GNOME, XFCE, Cinnamon, MATE, i3, and any other reasonably standards-compliant X11 DE. Multi-monitor virtual roots are rejected to avoid guessing which monitor we're on. |
| Blind 120 px reserve (everything else, including pure Wayland without XWayland) | Conservative fallback; matches a "Huge" KDE Plasma panel and a default macOS Dock so a misdetection still lands the modal above the taskbar in the common case. |

The lookup is cached process-wide after the first call; resizing your
panel without restarting Aura yields stale numbers, which is preferable to
hitting the file system / D-Bus on every resize frame.

## macOS Accessory mode

GPUI forces `NSApplicationActivationPolicyRegular` in `did_finish_launching`,
which would make Aura appear in Cmd+Tab and the Dock. For a tray-indicator
that's noise. `platform::apply_app_switcher_policy` flips the policy to
`Accessory` (menu-bar-only, no Dock icon, no Cmd+Tab) right after GPUI
finishes launching.

Users who want Aura in the Cmd+Tab list (e.g. to alt-tab to the modal) can
flip `display.show_in_app_switcher = true` in `config.toml` or use the
"Show in App Switcher" toggle in the settings panel — the change applies
immediately, no restart.

## File-handler dispatch

`platform::open_path()` opens a file with the OS default handler, falling
back to the system file manager (with the file pre-selected when supported)
if no handler is registered.

| Step | Linux                                                                  | macOS                       | Windows                                                                |
| ---- | ---------------------------------------------------------------------- | --------------------------- | ---------------------------------------------------------------------- |
| Open | `xdg-open <path>`                                                      | `open <path>`               | `ShellExecuteW(NULL, "open", path, ...)`                               |
| Fall back to reveal | `dbus-send … org.freedesktop.FileManager1.ShowItems` → `xdg-open <parent_dir>` | `open -R <path>` (Finder selects the file) | `explorer.exe /select,<path>`                                          |

`platform::open_url()` is the same dispatcher minus the file-manager
fallback (a missing browser is a different kind of broken). All work
happens on a detached thread so the GPUI click handler returns immediately.

## Why this pattern (cross-reference: Zed)

Zed ships a similar `Platform` trait with `open_with_system` and
`reveal_path` methods, dispatched to per-OS impls in `crates/gpui_linux/`,
`crates/gpui_macos/`, `crates/gpui_windows/`. The implementations are very
similar to what we have here — `xdg-open`, `open -R`, `ShellExecuteW`,
`SHOpenFolderAndSelectItems`. The main divergence is that Zed pulls in
`ashpd` for sandbox-aware portal-based reveals on Linux; Aura shells out
to `dbus-send` to avoid the dep (we never run sandboxed). That's a tradeoff
worth revisiting if Flatpak distribution lands on the roadmap.

## Adding a new platform

The pattern that emerged from the existing implementations:

1. **Tray backend** — if `ksni` or `tray-icon` already supports the platform,
   add it to the relevant cfg block in `tray.rs`. Otherwise the
   `TrayEvent`/`try_recv_event` contract is the seam to write a new
   backend against.
2. **Modal positioning** — add a `#[cfg(target_os = "newos")]` branch in
   `main.rs::compute_modal_bounds`. The Linux/Windows branch is the
   right starting point for any platform with a bottom-anchored tray.
3. **Single-instance** — add a cfg branch in
   `platform::acquire_single_instance`. The unix `flock` impl is the
   simplest and tends to work as-is on any new Unix.
4. **Work-area detection** — add a module under `work_area.rs` that
   returns the bottom reservation. If you can't find a native API, the
   blind 120 px reserve is fine until users complain.
5. **Document it here** — extend the tables above so the next port has
   the contract spelled out.
