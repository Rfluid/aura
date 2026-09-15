---
title: Platform integration — clickable tray icon
status: draft
version: 0.1.0
last_updated: 2026-09-13
last_verified: 2026-09-13
source_refs:
  - crates/aura/src/tray.rs
  - crates/aura/src/tray_status.rs
  - crates/aura/src/placement.rs
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

The two backends share a `TrayEvent` enum (`Show { anchor: Option<TrayAnchor> }`
/ `Quit`) and a `try_recv_event()` poller; the main loop drains events
every 150 ms regardless of platform. The same loop drains queued indicator
state (see [Live indicator state](#live-indicator-state)).

The Linux `ksni` backend uses `libayatana-appindicator`'s wire protocol but
talks D-Bus directly — `tray-icon`'s `gtk` feature refuses to surface
primary-click on AppIndicator hosts (it expects a menu and treats click as
"open menu"), so we picked an SNI-native implementation instead.

### Surviving a missing StatusNotifier host

`tray::install` spawns the ksni service with `assume_sni_available(true)`.
Without it, ksni's default is to fail `spawn()` outright when
`org.kde.StatusNotifierWatcher` is not on the bus — **and to start no service**,
so nothing ever retries. Aura would then keep running with no icon and no
window: invisible, and unreachable except through a task manager.

Two ordinary situations reach that branch:

- **Login race.** `aura.service` is ordered `After=graphical-session.target`,
  which does not wait for the panel to claim the watcher name. Whether the icon
  appears is then a coin flip per boot.
- **SNI arriving late.** A GNOME user enabling the AppIndicator extension after
  Aura has already started.

With the flag set, both route to `Tray::watcher_offline` (which logs and
returns `true` to keep the service alive) and ksni re-registers as soon as a
host appears — `Tray::watcher_online` logs the recovery. A genuine D-Bus
failure still returns `Err`, and `main()` responds by opening the modal once
so the session is not left with no UI at all.

### Icon rendering

The tray mark keeps the brand SVG's center dot but completes its surrounding
ring to make the progress model immediately familiar. A dim 360° track shows
the full extent and a solid arc fills to the highest current quota-window
usage. The mark is rasterised at every size in `tray::ICON_SIZES`
(16/22/24/32/48/64). Its color follows a stepped ramp: purple below 50%,
yellow from 50%, orange from 75%, and red from 90%. With no usable reading,
Aura renders the complete ring at full opacity instead of an empty gauge that
would falsely imply 0%.

Backends differ in what they want:

| Platform | Sizes handed over | Notes |
| --- | --- | --- |
| Linux | all of them, as `IconPixmap` | The SNI spec models the property as a list so the host can pick per panel size. `IconName` stays empty because hosts prefer a named, installed static asset over the live pixmaps, which would hide gauge updates. |
| macOS | one 64 px raster | AppKit draws the status item at 18 pt and downsamples; a dense source keeps a 2× menu bar sharp. The plain/no-reading state is a black template image that AppKit recolors for light/dark/highlight. A live gauge opts out of template mode so its usage color remains visible. |
| Windows | one raster at `SM_CXSMICON` for the current DPI | `Shell_NotifyIcon` blits rather than resamples, so rendering straight at the target size beats handing Win32 a 64 px icon to squeeze into 16. Odd DPI values snap up to the next size we rasterise. |

### Live indicator state

A tray icon that only opens a window is a button. `tray_status::summarize`
turns a `QuotaSnapshot` into a one-line tooltip (`"Claude · 5h 72% · week 31%"`)
and a whole-number peak usage value. `tray.rs` uses that value for both the
ring fill and color ramp. Peak usage considers every quota window, even those
omitted from the two-window tooltip.

Two producers feed `tray::set_status`:

1. `app.rs::apply_refresh_result` — free, since the modal just loaded a
   snapshot anyway.
2. `tray_status::spawn_poll` — a detached thread on a long interval
   (`display.tray_status_interval_secs`, default 1200 s, floored at 30 s) for
   the stretches when the modal is closed. Blocking HTTP, hence a thread
   rather than a GPUI task.

Both only *queue*; `TrayHandle::apply_pending_status` applies on the GPUI main
thread from the poll loop, because AppKit refuses `NSStatusItem` mutation from
anywhere else. It also diffs against the last applied value, so a poll that
produces identical numbers costs no D-Bus or AppKit traffic.

`display.tray_status` is the master switch for the background poll and all
live updates. `display.tray_progress` and `display.tray_color` independently
control the two drawn signals. `display.tray_pulse` is opt-in and maps usage at
or above 90% to SNI's `NeedsAttention` state on Linux; it has no equivalent on
macOS or Windows.

Each drawn signal carries its own reading (`TrayStatus::gauge_percent` and
`color_percent`), so the ring and the ramp watch different quota windows: by
default the ring fills from window 0 (the session on every backend Aura
speaks to) and the color climbs with window 1 (the week). One glance then
carries both the burst you're in and the budget you're spending, which a single
peak across all windows cannot.

The active agent's own `tray_progress_source` / `tray_color_source` repoint
either half at another position in the window list *that agent* reports. They
sit on `[[agents]]` rather than `[display]` because the positions are only
meaningful against one agent's windows. Backends push only the windows they
actually have (an idle Claude session has no 5h window; a plan without Opus has
no Opus week), so positions shift — a selector that lands past the end, or on a
token-only window with no percentage, falls back to the peak rather than
blanking the icon, which is also what puts the ramp on the single window a
one-window agent reports. The flip side of reading by position: a window
further down the list that is running out reaches the tooltip but not the icon.
`tray_pulse` follows the color reading, being the loud end of the same ramp.

## Modal positioning

`placement::modal_bounds()` decides where the modal opens; the choice is
OS-specific because that's where the tray icon lives:

- **macOS**: tray icon is at the **top** in the menu bar. Modal anchors
  ~25 pt below the bar, horizontally centred on the status item — using the
  icon's own rect from `TrayAnchor` when the backend reported one, and the
  click X otherwise.
- **Linux**: tray icon sits in the panel, usually bottom-right. The modal
  pins its bottom edge above the *work area* (display minus reserved panel
  space — see [Work-area detection](#work-area-detection)) and centres
  horizontally on the icon, the way Plasma's and GNOME's own systray popups
  do. The icon X comes from StatusNotifierItem's `Activate(x, y)` hint; the
  clamp in `placement::centered_x` keeps the window on screen when the icon
  is near an edge, which it almost always is.
- **Windows**: same bottom-anchored work-area placement, but right-aligned
  to the screen edge rather than centred — that's what the native volume /
  network flyouts do.
- **No position at all**: the tray menu's "Show Aura" item carries no click
  coordinates, so it falls back to the bottom-right corner everywhere.

### The modal opens at the height it last settled at

`AppState.modal_height` records the height the content fitted to, and the next
open starts there instead of at `placement::MODAL_H`. Without it the window is
created at a size its content will not have and positioned for *that* size —
which reads as the modal appearing and then jumping a frame later.

It matters most for `anchor = "none"`, which never repositions: opened at the
fallback height, the window would keep a top edge chosen for a 640px window and
float well clear of the panel for the whole session. Persisting rather than
keeping it in memory is what extends the fix to the *first* open after a
restart.

Two things make this safe to persist. Stale values self-correct, because the
auto-fit measures real content on the next frame regardless. And
`placement::fit_cap` holds back a `SCREEN_GAP` on both of its branches, so a
re-measurement of an unchanged window agrees with the height it was opened at —
when only the repositioning branch reserved the gap, each open measured 8px
more than the last, saved it, and the modal grew a little on every launch until
it reached the taskbar.

The write happens from the tray poll loop, not the layout callback: it is
change-gated, only ever records real content (placeholder measurements are
skipped while a refresh is in flight), and does a read-modify-write so a
profile picked in the modal isn't clobbered.

### Wayland has no client-side positioning

None of the above is possible on a native Wayland surface: `xdg_toplevel`
has no position in the protocol, so the compositor places the modal, the
requested origin is discarded, and `platform::set_window_origin` has no
window id to move. `display.anchor`, taskbar avoidance and icon-centring all
go quiet at once — and an auto-hidden panel will happily slide out over the
modal, because nothing reserved space for it.

`platform::select_display_backend` handles this at startup. GPUI picks its
Linux backend inside `guess_compositor()`, which returns `"Wayland"` whenever
`$WAYLAND_DISPLAY` is non-empty and offers no override, so Aura hides that
variable for exactly the duration of `Application::new()` and restores it
from the returned guard's `Drop`. GPUI connects over XWayland, where all the
positioning above works, and child processes (plugin commands, `xdg-open`)
still inherit the session's real environment.

`display.linux_backend` controls it: `"auto"` (default) prefers X11 whenever
`$DISPLAY` resolves, `"x11"` additionally warns when it doesn't, `"wayland"`
opts back into the native backend for users who find XWayland soft on a
fractional-scale display. The decision table is a pure function,
`platform::backend_choice`, so it's unit-tested without touching the process
environment. Those users want a compositor window rule instead: KDE matches
on `WM_CLASS = aura`, Position → Apply Initially.

### Two coordinate-space traps

Both are invisible on a single, non-HiDPI monitor, which is why they survived
so long:

1. **tray-icon reports physical pixels.** macOS multiplies the AppKit point by
   the status bar window's `backingScaleFactor`; Windows passes `GetCursorPos`
   / `Shell_NotifyIconGetRect` through unchanged. GPUI reports display bounds
   in *logical* pixels. Unconverted, a click on a 2× Retina display yields an X
   twice as large as the real one, which the clamp in `modal_origin` then pins
   to the screen corner — so the popover never tracked the icon at all.
   `platform::tray_scale_factor` supplies the divisor (per-monitor DPI via
   `MonitorFromPoint` + `GetDpiForMonitor` on Windows; `NSScreen.screens[0]`'s
   `backingScaleFactor` on macOS, where the menu bar lives), and `tray.rs`
   converts at the edge so nothing downstream handles physical pixels.

2. **GPUI's window origin is not absolute on macOS.** `open_window` adds the
   target `NSScreen`'s frame origin back onto `bounds.origin`, so it expects a
   *display-relative* coordinate; Windows and X11 expect an absolute one.
   `placement::to_window_origin` does that conversion at the single point where
   it matters — window creation. The post-resize move in `app.rs` goes through
   `platform::set_window_origin` (`setFrameTopLeftPoint` / `ConfigureWindow` /
   `SetWindowPos`), all of which are absolute everywhere, so that path is left
   alone.

### Multi-monitor

`placement::modal_display` picks the display whose bounds contain the tray
anchor, falling back to the primary. Its `DisplayId` is then used twice:

- passed to `WindowOptions::display_id`. Not optional on Windows — `open_window`
  validates the requested bounds against this display (the primary one when
  unset) and silently substitutes its centred default when they don't fit,
  which is exactly what a secondary-monitor origin looks like;
- stored on `AuraView`, so the auto-fit callback caps the modal's height
  against *this* screen's work area rather than the primary's taskbar.

GPUI's macOS display backend hard-codes `origin: Default::default()` and
returns only the size, so every screen claims to start at `(0, 0)` —
useless for deciding which one a click landed on. `platform::display_bounds`
recovers the real origin from `CGDisplayBounds` and is a pass-through on the
backends that already report one.

## Dismissal

Two ways out, both handled by the same poll loop because it owns the window
handle.

**Escape.** A keystroke observer registered in the `run` closure closes the
modal, which is the convention for a tray popup on every desktop. The
selectable-text bridge is installed with `clear_on_escape: false` and Escape is
handled in one place instead, because the two meanings have to be ordered:
Escape clears a live text selection, and only closes the popup when there is
nothing to clear. As two independent keystroke observers the outcome would
depend on subscriber iteration order, and a single Escape could clear *and*
close. Unlike focus loss, Escape is honoured regardless of
`dismiss_on_focus_loss` or an in-flight plugin action — it is an explicit
instruction.

**Click-outside.** The main poll loop polls `cx.active_window()` every 150 ms;
when it returns `None` (no GPUI window is the OS-foreground window) the modal
is closed.

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

## Show-in-app-switcher (cross-platform)

`display.show_in_app_switcher` controls whether Aura's modal appears in
each OS's "where are my windows" surfaces:

| Platform | `false` (default)                                        | `true`                                                          |
| -------- | -------------------------------------------------------- | --------------------------------------------------------------- |
| macOS    | `NSApplicationActivationPolicyAccessory` (menu-bar only) | `NSApplicationActivationPolicyRegular` (Cmd+Tab + Dock)         |
| Windows  | `WindowKind::PopUp` → `WS_EX_TOOLWINDOW` (no taskbar entry) | `WindowKind::Normal` → `WS_EX_APPWINDOW` (Alt+Tab + taskbar)    |
| Linux    | `WindowKind::PopUp` (skipped by most DEs' window list)   | `WindowKind::Normal` (visible to the panel / window switcher)   |

The macOS policy is process-wide and is applied at startup *and* on every
modal refresh, so editing the field in `config.toml` and clicking the
refresh icon (or just reopening the modal) picks up the new value
without a service restart. On Windows / Linux the field is read at modal
open time — clicking the tray icon to reopen swaps the `WindowKind`.

GPUI forces `NSApplicationActivationPolicyRegular` in
`did_finish_launching`, which would override our setting on first launch.
`platform::apply_app_switcher_policy` is therefore reapplied inside the
GPUI `run` closure (after launching completes) to win the race.

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
   `placement::modal_origin`. The Linux/Windows branch is the
   right starting point for any platform with a bottom-anchored tray.
3. **Single-instance** — add a cfg branch in
   `platform::acquire_single_instance`. The unix `flock` impl is the
   simplest and tends to work as-is on any new Unix.
4. **Work-area detection** — add a module under `work_area.rs` that
   returns the bottom reservation. If you can't find a native API, the
   blind 120 px reserve is fine until users complain.
5. **Document it here** — extend the tables above so the next port has
   the contract spelled out.
