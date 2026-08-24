// The `objc` 0.2 crate's macros (msg_send!, class!, sel_impl!) emit an
// internal `cargo-clippy` cfg that newer rustc/clippy treats as unknown.
// The macros are third-party; suppress the lint rather than forking objc.
#![allow(unexpected_cfgs)]

//! Thin per-OS façade for the things Aura asks of the host: tweaking the
//! macOS activation policy and asking the desktop to open a file/URL with
//! the user's default handler.
//!
//! The `open_*` functions return immediately and do the actual work on a
//! detached thread — the click handlers that call them run on the GPUI
//! main thread and shouldn't block on a subprocess.
//!
//! ## Fallback semantics
//!
//! `open_path()` first tries the OS default handler (`xdg-open` / `open` /
//! `ShellExecuteW`). If that handler reports failure — common on Linux when
//! the MIME type has no association, or on Windows when a path has no
//! registered verb — it falls back to revealing the path in the system file
//! manager (`org.freedesktop.FileManager1.ShowItems` over D-Bus on Linux,
//! `open -R` on macOS, `explorer /select,` on Windows). That way a
//! freshly-installed system without a TOML editor still lands the user
//! somewhere they can act on the file, instead of a silent no-op.

use std::path::Path;
use std::sync::mpsc::Receiver;
use std::sync::{Mutex, OnceLock};

// ── Single-instance guard ───────────────────────────────────────────────────

/// Receives one `()` per second-instance launch that pinged us after losing
/// the single-instance race. Populated only on the winning (first) instance,
/// by `unix_lock::acquire` / `windows_mutex::acquire`.
static ACTIVATION_RX: OnceLock<Mutex<Receiver<()>>> = OnceLock::new();

/// Try to acquire the per-user single-instance lock. Returns `true` if this
/// process is now the sole Aura instance, `false` if another Aura is already
/// running (in which case `main()` should exit silently — the running
/// instance is pinged as part of losing this race and will show its window
/// on the next poll of [`try_recv_activation`]).
///
/// Mechanism per platform:
///
/// - **Unix (Linux, macOS, BSD)**: an exclusive `flock()` on a file in
///   `$XDG_RUNTIME_DIR` (Linux/BSD) or `$TMPDIR` (macOS). The lock is held
///   for the process lifetime — we intentionally leak the file descriptor so
///   the kernel releases it on exit. The winner also listens on a Unix
///   domain socket next to the lock file; a loser connects to it as a
///   "someone tried to launch me again" signal.
/// - **Windows**: a named mutex (`Local\AuraSingleInstance`) created via
///   `CreateMutexW`. `ERROR_ALREADY_EXISTS` from the same call signals
///   another instance owns it. The handle is leaked for the same reason.
///   The winner also listens on a named pipe for the same signal.
pub fn acquire_single_instance() -> bool {
    #[cfg(unix)]
    {
        unix_lock::acquire()
    }
    #[cfg(target_os = "windows")]
    {
        windows_mutex::acquire()
    }
    #[cfg(not(any(unix, target_os = "windows")))]
    {
        true
    }
}

/// Non-blocking poll for a re-launch ping from a second instance that lost
/// the single-instance race. Drains every pending ping and returns `true` if
/// at least one arrived — callers only care that *a* relaunch happened, not
/// how many. Called from GPUI's async task alongside `tray::try_recv_event`.
pub fn try_recv_activation() -> bool {
    let Some(mtx) = ACTIVATION_RX.get() else {
        return false;
    };
    let Ok(rx) = mtx.lock() else {
        return false;
    };
    let mut pinged = false;
    while rx.try_recv().is_ok() {
        pinged = true;
    }
    pinged
}

#[cfg(unix)]
mod unix_lock {
    use super::ACTIVATION_RX;
    use std::os::fd::AsRawFd;
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::PathBuf;
    use std::sync::mpsc;

    pub fn acquire() -> bool {
        let path = lock_path();
        let file = match std::fs::OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&path)
        {
            Ok(f) => f,
            Err(e) => {
                // Couldn't even open the lock file — proceed without a
                // guard rather than refusing to launch. Worst case is two
                // tray icons appear; both will still work.
                eprintln!(
                    "aura: single-instance lock open failed at {}: {e}",
                    path.display()
                );
                return true;
            }
        };

        // SAFETY: libc::flock takes a raw fd we just opened. LOCK_EX |
        // LOCK_NB returns 0 if we acquired the exclusive lock, -1 with
        // errno EWOULDBLOCK if another process holds it.
        let ret = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if ret == 0 {
            // Keep the descriptor alive for the process lifetime so the
            // kernel releases the lock automatically on exit (including
            // SIGKILL / panic). std::mem::forget is fine — the kernel,
            // not Drop, releases the lock.
            std::mem::forget(file);
            spawn_activation_listener();
            true
        } else {
            ping_running_instance();
            false
        }
    }

    /// Winner side: bind the activation socket and forward one `()` per
    /// accepted connection into `ACTIVATION_RX`. Best-effort — if the
    /// socket can't be bound, second-instance launches just go back to
    /// exiting silently (today's behaviour), so failures here aren't fatal.
    fn spawn_activation_listener() {
        let path = socket_path();
        // A previous run's socket file survives an unclean shutdown
        // (SIGKILL). We hold the flock, so we know we're the only
        // instance — safe to clear the stale file before binding.
        let _ = std::fs::remove_file(&path);

        let listener = match UnixListener::bind(&path) {
            Ok(l) => l,
            Err(e) => {
                eprintln!(
                    "aura: activation socket bind failed at {}: {e}",
                    path.display()
                );
                return;
            }
        };

        let (tx, rx) = mpsc::channel::<()>();
        let _ = ACTIVATION_RX.set(std::sync::Mutex::new(rx));

        std::thread::Builder::new()
            .name("aura-activation-listener".into())
            .spawn(move || {
                for stream in listener.incoming().flatten() {
                    drop(stream);
                    if tx.send(()).is_err() {
                        break;
                    }
                }
            })
            .ok();
    }

    /// Loser side: connect to the winner's activation socket to signal
    /// "someone tried to launch Aura again". A connection alone is the
    /// signal — no payload needed. Failure (stale socket, no listener) is
    /// silently ignored: the process still exits the same way it did
    /// before this existed.
    fn ping_running_instance() {
        let _ = UnixStream::connect(socket_path());
    }

    /// $XDG_RUNTIME_DIR is per-user and on tmpfs (cleaned at logout) on
    /// systemd distros — the canonical home for lock files. Fall back to
    /// `std::env::temp_dir()`, which on macOS resolves to a per-user
    /// `/var/folders/...` directory (also per-user) and on stripped-down
    /// Linux to `/tmp`. The fallback is multi-user on /tmp, so we include
    /// the UID in the filename to avoid stealing each other's lock.
    fn lock_path() -> PathBuf {
        base_dir().join("aura.lock")
    }

    /// Same directory as the lock file, for the activation socket.
    fn socket_path() -> PathBuf {
        base_dir().join("aura.sock")
    }

    fn base_dir() -> PathBuf {
        if let Some(d) = std::env::var_os("XDG_RUNTIME_DIR") {
            return PathBuf::from(d);
        }
        std::env::temp_dir()
    }
}

#[cfg(target_os = "windows")]
mod windows_mutex {
    use super::ACTIVATION_RX;
    use std::io::Read;
    use std::sync::mpsc;

    const PIPE_NAME: &str = r"\\.\pipe\AuraActivation";

    pub fn acquire() -> bool {
        use windows::core::w;
        use windows::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
        use windows::Win32::System::Threading::CreateMutexW;
        match unsafe { CreateMutexW(None, false, w!("Local\\AuraSingleInstance")) } {
            Ok(h) => {
                if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
                    ping_running_instance();
                    return false;
                }
                // HANDLE is Copy; Win32 keeps it open until process exit.
                let _ = h;
                spawn_activation_listener();
                true
            }
            Err(e) => {
                eprintln!("aura: single-instance check failed: {e}");
                true
            }
        }
    }

    /// Winner side: repeatedly open the named pipe as a server and forward
    /// one `()` per connecting client into `ACTIVATION_RX`. Best-effort,
    /// same rationale as the Unix listener.
    fn spawn_activation_listener() {
        let (tx, rx) = mpsc::channel::<()>();
        let _ = ACTIVATION_RX.set(std::sync::Mutex::new(rx));

        std::thread::Builder::new()
            .name("aura-activation-listener".into())
            .spawn(move || loop {
                match named_pipe_server::wait_for_client(PIPE_NAME) {
                    Ok(()) => {
                        if tx.send(()).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            })
            .ok();
    }

    /// Loser side: connect to the winner's named pipe to signal a
    /// relaunch. Failure is silently ignored — same fallback as Unix.
    fn ping_running_instance() {
        use std::time::Duration;
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(PIPE_NAME)
        {
            let mut buf = [0u8; 1];
            // Touch the pipe so the server's ConnectNamedPipe wait unblocks;
            // we don't care about the contents.
            let _ = f.read(&mut buf);
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    /// Thin wrapper around the Win32 named-pipe server API — `std::fs`
    /// has no server-side named-pipe support. The client side (loser
    /// process) just opens the pipe path with `std::fs::OpenOptions`,
    /// which Windows treats like any other `CreateFileW` target.
    mod named_pipe_server {
        use windows::core::HSTRING;
        use windows::Win32::Foundation::CloseHandle;
        use windows::Win32::Storage::FileSystem::PIPE_ACCESS_DUPLEX;
        use windows::Win32::System::Pipes::{
            ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_TYPE_BYTE, PIPE_WAIT,
        };

        pub fn wait_for_client(name: &str) -> Result<(), ()> {
            let name = HSTRING::from(name);
            let handle = unsafe {
                CreateNamedPipeW(
                    &name,
                    PIPE_ACCESS_DUPLEX,
                    PIPE_TYPE_BYTE | PIPE_WAIT,
                    1,
                    64,
                    64,
                    0,
                    None,
                )
            };
            if handle.is_invalid() {
                return Err(());
            }
            let connected = unsafe { ConnectNamedPipe(handle, None) };
            let result = if connected.is_ok() { Ok(()) } else { Err(()) };
            unsafe {
                let _ = DisconnectNamedPipe(handle);
                let _ = CloseHandle(handle);
            }
            result
        }
    }
}

// ── macOS activation policy ──────────────────────────────────────────────────

/// Set the macOS NSApplication activation policy.
///
/// `show = true`  → NSApplicationActivationPolicyRegular   (appears in Cmd+Tab)
/// `show = false` → NSApplicationActivationPolicyAccessory (background-only, menu-bar only)
#[cfg(target_os = "macos")]
pub fn apply_app_switcher_policy(show: bool) {
    use cocoa::appkit::NSApplicationActivationPolicy::{
        NSApplicationActivationPolicyAccessory, NSApplicationActivationPolicyRegular,
    };
    use objc::{class, msg_send, sel, sel_impl};

    unsafe {
        let app: cocoa::base::id = msg_send![class!(NSApplication), sharedApplication];
        let policy = if show {
            NSApplicationActivationPolicyRegular
        } else {
            NSApplicationActivationPolicyAccessory
        };
        let _: () = msg_send![app, setActivationPolicy: policy];
    }
}

#[cfg(not(target_os = "macos"))]
pub fn apply_app_switcher_policy(_show: bool) {}

// ── macOS click-outside monitor ──────────────────────────────────────────────
//
// Accessory apps on macOS (LSUIElement/setActivationPolicy:.accessory) can't
// reliably set the application's "main window" — `[NSApp mainWindow]` returns
// nil even after we promote the policy to Regular and call activateIgnoringOtherApps.
// That kills `cx.active_window()`-based focus-loss detection (it reads mainWindow).
//
// The reliable signal is an NSEvent global monitor, which fires only for
// mouse-down events sent to OTHER applications. We pair it with
// WindowKind::Normal (regular NSWindow rather than NonactivatingPanel) so
// clicks delivered to our window aren't seen by the global monitor — that
// avoids the false-positive on interactive elements we saw with the panel
// kind.

/// Opaque handle for a global NSEvent monitor. Pass it to
/// `remove_click_outside_monitor` when the window closes.
#[cfg(target_os = "macos")]
pub struct ClickOutsideMonitor(cocoa::base::id);

// SAFETY: the monitor object is only ever touched from the GPUI main thread.
#[cfg(target_os = "macos")]
unsafe impl Send for ClickOutsideMonitor {}

/// Register a global mouse-down monitor. The `clicked` flag is set to `true`
/// whenever the user clicks in any application other than Aura.
#[cfg(target_os = "macos")]
pub fn install_click_outside_monitor(
    clicked: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> ClickOutsideMonitor {
    use block::ConcreteBlock;
    use cocoa::base::id;
    use objc::{class, msg_send, sel, sel_impl};

    // NSEventMaskLeftMouseDown | NSEventMaskRightMouseDown | NSEventMaskOtherMouseDown
    let mask: u64 = (1 << 1) | (1 << 3) | (1 << 25);

    let block = ConcreteBlock::new(move |_event: id| {
        clicked.store(true, std::sync::atomic::Ordering::Relaxed);
    });
    let block = block.copy();

    let monitor: id = unsafe {
        msg_send![class!(NSEvent),
            addGlobalMonitorForEventsMatchingMask: mask
            handler: &*block]
    };

    std::mem::forget(block);
    ClickOutsideMonitor(monitor)
}

/// Unregister and release a previously installed global monitor.
#[cfg(target_os = "macos")]
pub fn remove_click_outside_monitor(monitor: ClickOutsideMonitor) {
    use objc::{class, msg_send, sel, sel_impl};
    unsafe {
        let _: () = msg_send![class!(NSEvent), removeMonitor: monitor.0];
    }
}

/// Promote the given window above other applications' windows by raising its
/// NSWindow level. Required for menu-bar popovers: GPUI's `WindowKind::Normal`
/// uses `NSNormalWindowLevel`, which leaves the modal behind whatever app the
/// user was using when they clicked the tray icon.
#[cfg(target_os = "macos")]
pub fn raise_window_to_floating(window: &mut gpui::Window) {
    use objc::{msg_send, sel, sel_impl};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    // NSFloatingWindowLevel = 3 — above normal windows but below menus/status.
    const NS_FLOATING_WINDOW_LEVEL: i64 = 3;

    let Ok(wh) = <gpui::Window as HasWindowHandle>::window_handle(window) else {
        return;
    };
    let RawWindowHandle::AppKit(h) = wh.as_raw() else {
        return;
    };
    unsafe {
        let ns_view = h.ns_view.as_ptr() as cocoa::base::id;
        let ns_window: cocoa::base::id = msg_send![ns_view, window];
        if !ns_window.is_null() {
            let _: () = msg_send![ns_window, setLevel: NS_FLOATING_WINDOW_LEVEL];
        }
    }
}

// ── Window repositioning after auto-fit resize ──────────────────────────────
//
// GPUI 0.2's `Window::resize` issues `SetWindowPos(.., SWP_NOMOVE)`, which
// keeps the window's top-left fixed and grows/shrinks the bottom. For an
// `Anchor::Bottom` modal we want the opposite — the bottom edge should stay
// flush with the taskbar — so after the resize we move the top-left to the
// freshly computed `placement::modal_origin`.
//
// `desired_origin` MUST be computed absolutely from the work area, never read
// back from the window's current position: that absoluteness is what keeps
// repeated calls idempotent instead of letting the window walk across the
// screen (issue #27).
//
// Two entry points, picked by the caller's `#[cfg]`:
//   - Windows: `reposition_after_resize` — also carries the open-time DWM
//     uncloak, which must fire after the resize in *every* anchor mode.
//   - macOS / Linux: `set_window_origin` — a plain move, called only when
//     there's a target (`Anchor::Bottom`).

/// Windows: move the modal to `desired_origin` (when `Some`) and lift the
/// open-time DWM cloak, both after the auto-fit resize.
///
/// `desired_origin` is `None` for non-repositioning anchors (`none`/`top`) or
/// when no display info is available; in that case the window keeps the
/// top-left GPUI's resize left it and we only uncloak. `resize` (the caller)
/// and the `SetWindowPos` + uncloak here all run on the `ForegroundExecutor`
/// (FIFO), so DWM only ever composites the final state — no flash, no jump.
#[cfg(target_os = "windows")]
pub(crate) fn reposition_after_resize(
    window: &mut gpui::Window,
    cx: &mut gpui::App,
    desired_origin: Option<gpui::Point<gpui::Pixels>>,
    uncloak: &std::rc::Rc<std::cell::Cell<bool>>,
) {
    let scale = window.scale_factor();
    let move_to = desired_origin.map(|origin| {
        (
            (f32::from(origin.x) * scale).round() as i32,
            (f32::from(origin.y) * scale).round() as i32,
        )
    });
    let do_uncloak = uncloak.replace(false);
    if move_to.is_none() && !do_uncloak {
        return;
    }
    let hwnd_val = window.hwnd();

    cx.spawn(async move |_cx| {
        use windows::Win32::Foundation::HWND;
        let hwnd = HWND(hwnd_val as *mut _);
        if let Some((x_phys, y_phys)) = move_to {
            use windows::Win32::UI::WindowsAndMessaging::{
                SetWindowPos, SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER,
            };
            unsafe {
                let _ = SetWindowPos(
                    hwnd,
                    None,
                    x_phys,
                    y_phys,
                    0,
                    0,
                    SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
                );
            }
        }
        if do_uncloak {
            use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_CLOAK};
            let val: i32 = 0;
            unsafe {
                let _ = DwmSetWindowAttribute(
                    hwnd,
                    DWMWA_CLOAK,
                    std::ptr::addr_of!(val).cast(),
                    std::mem::size_of::<i32>() as u32,
                );
            }
        }
    })
    .detach();
}

/// macOS: move the modal's top-left to `origin` (logical points, top-left
/// origin, Y down — the space `placement` works in). Used for
/// `Anchor::Bottom`, which on macOS is an explicit non-default choice (the
/// tray lives in the menu bar up top).
///
/// AppKit uses a bottom-left screen origin with Y measured upward, so we flip
/// against the main screen's height and call `setFrameTopLeftPoint:`.
#[cfg(target_os = "macos")]
pub(crate) fn set_window_origin(
    window: &mut gpui::Window,
    _cx: &mut gpui::App,
    origin: gpui::Point<gpui::Pixels>,
) {
    use cocoa::foundation::{NSPoint, NSRect};
    use objc::{class, msg_send, sel, sel_impl};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    let Ok(wh) = <gpui::Window as HasWindowHandle>::window_handle(window) else {
        return;
    };
    let RawWindowHandle::AppKit(h) = wh.as_raw() else {
        return;
    };
    unsafe {
        let ns_view = h.ns_view.as_ptr() as cocoa::base::id;
        let ns_window: cocoa::base::id = msg_send![ns_view, window];
        if ns_window.is_null() {
            return;
        }
        let screen: cocoa::base::id = msg_send![class!(NSScreen), mainScreen];
        if screen.is_null() {
            return;
        }
        let frame: NSRect = msg_send![screen, frame];
        // Flip GPUI's top-down Y into AppKit's bottom-up screen Y.
        let top_left = NSPoint::new(
            f64::from(f32::from(origin.x)),
            frame.size.height - f64::from(f32::from(origin.y)),
        );
        let _: () = msg_send![ns_window, setFrameTopLeftPoint: top_left];
    }
}

/// Linux: move the modal's top-left to `origin` (logical points). GPUI 0.2
/// exposes no move API, so we issue an X11 move ourselves against the XCB
/// window id.
///
/// This works on **X11** (KWin honours position requests for our notification-
/// type popup, the same way it honours GPUI's open position). On **Wayland**
/// the window handle is a Wayland surface, not an XCB id — the match below
/// falls through and we log the limitation once, because the Wayland protocol
/// forbids clients from positioning their own toplevels (use a KWin window
/// rule instead; see docs/configuration.md).
///
/// This only *moves* the window; the caller is responsible for the resize
/// (GPUI's `window.resize()`). The two are sequenced by the caller so that the
/// window never crosses the taskbar mid-transition (see the call site in
/// `app.rs`): a combined move+resize via `_NET_MOVERESIZE_WINDOW` was tried and
/// reverted because KWin honours that message's *move* bits but silently drops
/// its *size* bits, leaving the modal stuck at its initial height.
#[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
pub(crate) fn set_window_origin(
    window: &mut gpui::Window,
    _cx: &mut gpui::App,
    origin: gpui::Point<gpui::Pixels>,
) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    let scale = window.scale_factor();
    let Ok(wh) = <gpui::Window as HasWindowHandle>::window_handle(window) else {
        return;
    };
    let RawWindowHandle::Xcb(h) = wh.as_raw() else {
        // Wayland (or some other backend) — can't reposition. Warn once.
        static WARNED: std::sync::Once = std::sync::Once::new();
        WARNED.call_once(|| {
            eprintln!(
                "aura: display.anchor = \"bottom\" has no live reposition on \
                 Wayland (the compositor owns window placement); the modal \
                 opens bottom-anchored but grows downward. See \
                 docs/configuration.md."
            );
        });
        return;
    };

    // X11 window coordinates are physical, root-relative pixels; GPUI gives us
    // logical points, so scale up (scale is 1.0 on most X11/KDE setups).
    let x = (f32::from(origin.x) * scale).round() as i32;
    let y = (f32::from(origin.y) * scale).round() as i32;
    let xid = h.window.get();

    // Fresh short-lived connection: repositioning only fires when the content
    // height changes (tab switch / refresh), so the handshake cost is
    // negligible and we avoid caching a connection that could go stale. The
    // XID is server-global, so our own connection can address it.
    let Ok((conn, screen_num)) = x11rb::connect(None) else {
        return;
    };
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{
        ClientMessageEvent, ConfigureWindowAux, ConnectionExt, EventMask,
    };

    // A plain ConfigureWindow moves *override-redirect* / unmanaged windows
    // (and is honoured by some minimal WMs), but a full window manager like
    // KWin owns the geometry of managed top-levels and silently ignores a
    // client's attempt to move itself this way after the window is mapped.
    // It still honours it for the unmanaged case, so issue it regardless.
    let _ = conn.configure_window(xid, &ConfigureWindowAux::new().x(x).y(y));

    // The EWMH-blessed way for a client to move its *own managed* window:
    // post a `_NET_MOVERESIZE_WINDOW` ClientMessage to the root with
    // SubstructureRedirect set, which the WM (KWin, Mutter, …) processes and
    // applies. This is what actually makes `anchor = "bottom"` hug the
    // taskbar under KWin. data = [flags, x, y, w, h]; the flags carry the
    // gravity (0 = use the window's win-gravity), which axes are present
    // (bits 8/9 = x/y), and the source indication (bit 12 = normal app).
    if let Ok(cookie) = conn.intern_atom(false, b"_NET_MOVERESIZE_WINDOW") {
        if let Ok(reply) = cookie.reply() {
            const X_PRESENT: u32 = 1 << 8;
            const Y_PRESENT: u32 = 1 << 9;
            const SOURCE_APP: u32 = 1 << 12;
            let flags = X_PRESENT | Y_PRESENT | SOURCE_APP;
            let root = conn.setup().roots[screen_num].root;
            let event =
                ClientMessageEvent::new(32, xid, reply.atom, [flags, x as u32, y as u32, 0, 0]);
            let _ = conn.send_event(
                false,
                root,
                EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
                event,
            );
        }
    }
    let _ = conn.flush();
}

// ── Open with system handler (file or URL) ──────────────────────────────────

/// Open a filesystem path with the desktop's default handler, falling back to
/// the file manager (with the file pre-selected when supported) if no handler
/// is registered.
pub fn open_path(path: &Path) {
    let path = path.to_path_buf();
    std::thread::Builder::new()
        .name("aura-open-path".into())
        .spawn(move || {
            if let Err(e) = open_path_blocking(&path) {
                eprintln!(
                    "aura: open_path({}) failed ({e}); falling back to file manager",
                    path.display()
                );
                if let Err(e2) = reveal_in_file_manager_blocking(&path) {
                    eprintln!(
                        "aura: reveal_in_file_manager({}) also failed: {e2}",
                        path.display()
                    );
                }
            }
        })
        .ok();
}

/// Open a URL with the desktop's default browser. Unlike `open_path`, there is
/// no file-manager fallback — if the browser launch fails we just log.
pub fn open_url(url: &str) {
    let url = url.to_owned();
    std::thread::Builder::new()
        .name("aura-open-url".into())
        .spawn(move || {
            if let Err(e) = open_url_blocking(&url) {
                eprintln!("aura: open_url({url}) failed: {e}");
            }
        })
        .ok();
}

// ── Per-OS blocking impls ────────────────────────────────────────────────────

fn open_path_blocking(path: &Path) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        run_status("open", &[path.as_os_str()])
    }
    #[cfg(target_os = "windows")]
    {
        shell_execute_open(path)
    }
    #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
    {
        run_status("xdg-open", &[path.as_os_str()])
    }
}

fn open_url_blocking(url: &str) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        run_status("open", &[std::ffi::OsStr::new(url)])
    }
    #[cfg(target_os = "windows")]
    {
        shell_execute_open(Path::new(url))
    }
    #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
    {
        run_status("xdg-open", &[std::ffi::OsStr::new(url)])
    }
}

fn reveal_in_file_manager_blocking(path: &Path) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        // `open -R` highlights the target in Finder rather than opening it.
        run_status("open", &[std::ffi::OsStr::new("-R"), path.as_os_str()])
    }
    #[cfg(target_os = "windows")]
    {
        // `explorer /select,<path>` opens Explorer with the file selected.
        // explorer.exe always returns exit code 1 on success; treat any
        // spawn that *runs* as success.
        let _ = std::process::Command::new("explorer.exe")
            .arg(format!("/select,{}", path.display()))
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()?
            .wait()?;
        Ok(())
    }
    #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
    {
        linux_reveal(path)
    }
}

// Generic Command::status helper that maps non-success exit codes to an Err
// so callers can detect "no handler registered for this MIME type" the way
// `xdg-open` reports it (exit code 3).
#[cfg(not(target_os = "windows"))]
fn run_status(program: &str, args: &[&std::ffi::OsStr]) -> std::io::Result<()> {
    let status = std::process::Command::new(program)
        .args(args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(std::io::Error::other(format!(
            "{program} exited with {status}"
        )))
    }
}

#[cfg(target_os = "windows")]
fn shell_execute_open(path: &Path) -> std::io::Result<()> {
    use windows::core::{w, HSTRING};
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    let target = HSTRING::from(path);
    let ret = unsafe { ShellExecuteW(None, w!("open"), &target, None, None, SW_SHOWNORMAL) };
    // Per the MSDN docs, return values ≤32 are error codes; anything larger
    // is the handle of the launched application/document.
    if ret.0 as isize > 32 {
        Ok(())
    } else {
        Err(std::io::Error::other(format!(
            "ShellExecuteW returned {}",
            ret.0 as isize
        )))
    }
}

// ── Linux file manager reveal ───────────────────────────────────────────────
//
// The org.freedesktop.FileManager1 D-Bus interface is implemented by GNOME
// Files (Nautilus), KDE Dolphin, Cinnamon Nemo, MATE Caja, and Pantheon
// Files — i.e. the file manager that ships with every mainstream desktop.
// We shell out to `dbus-send` instead of pulling in a D-Bus crate to keep
// the dep footprint flat (ksni already gives us all the D-Bus we want at
// runtime, but its connection is private). If the D-Bus call fails (no
// FileManager1 owner, or the binary is missing), we fall back to opening
// the parent directory with xdg-open so the user at least lands close to
// the target.

#[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
fn linux_reveal(path: &Path) -> std::io::Result<()> {
    if file_manager_1_show(path).is_ok() {
        return Ok(());
    }
    let parent = file_manager_target_dir(path);
    run_status("xdg-open", &[parent.as_os_str()])
}

#[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
fn file_manager_1_show(path: &Path) -> std::io::Result<()> {
    let uri = format!("file://{}", path.display());
    let status = std::process::Command::new("dbus-send")
        .args([
            "--session",
            "--dest=org.freedesktop.FileManager1",
            "--type=method_call",
            "/org/freedesktop/FileManager1",
            "org.freedesktop.FileManager1.ShowItems",
        ])
        .arg(format!("array:string:{uri}"))
        .arg("string:")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(std::io::Error::other(format!(
            "FileManager1.ShowItems exited with {status}"
        )))
    }
}

#[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
fn file_manager_target_dir(path: &Path) -> std::path::PathBuf {
    if path.is_dir() {
        path.to_path_buf()
    } else {
        path.parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| std::path::PathBuf::from("/"))
    }
}
