//! System-tray indicator.
//!
//! Linux uses [`ksni`] — a direct StatusNotifierItem implementation —
//! because it surfaces `Activate()` (primary-click) as a callback, which
//! gives us single-click open/close. tray-icon's libayatana-appindicator
//! backend hides primary-click events behind the context menu, costing a
//! second click to actually open the modal.
//!
//! macOS / Windows still use [`tray_icon`]: those backends already get
//! single-click activation natively via AppKit / Win32.
//!
//! Both backends feed a unified [`TrayEvent`] stream that `main.rs` drains
//! from the GPUI side via [`try_recv_event`], and both accept live state
//! pushes through [`set_status`] / [`apply_pending_status`] so the icon can
//! act as a real indicator instead of a static launcher.
//!
//! Middle-click (`secondary_activate` on SNI) opens the modal too.

use std::sync::Mutex;

use anyhow::{Context, Result};
use resvg::{tiny_skia, usvg};

use crate::assets::AURA_LOGO_SVG;

/// Icon sizes we rasterise, smallest first.
///
/// StatusNotifierItem hosts are handed the whole set and pick the one that
/// best fits the panel (the spec explicitly models `IconPixmap` as a list of
/// sizes). macOS and Windows want a single bitmap, so those backends pick one
/// entry — see `non_linux::macos_icon_size` / `non_linux::windows_icon_size`.
const ICON_SIZES: &[u32] = &[16, 22, 24, 32, 48, 64];

/// Largest entry in [`ICON_SIZES`], used as the "when in doubt, go big"
/// fallback for backends that scale for us. Only the tray-icon backend picks a
/// single size, so this is unread on Linux.
#[cfg_attr(target_os = "linux", allow(dead_code))]
const ICON_SIZE_MAX: u32 = 64;

/// Aura purple — must stay in sync with `app.rs::COLOR_ACCENT` so the tray
/// icon matches the in-app brand color. Unused on macOS outside tests, where
/// the normal-state icon is a template image instead (see
/// `ICON_COLOR_TEMPLATE`).
#[cfg_attr(target_os = "macos", allow(dead_code))]
const ICON_COLOR: &str = "#8b5cf6";

/// Icon color for the "needs attention" state (quota nearly exhausted).
/// Matches the danger color used by the modal's progress bars.
const ICON_COLOR_ATTENTION: &str = "#ef4444";

/// macOS template images are drawn from their alpha channel alone — AppKit
/// recolors them for the current menu-bar appearance (light / dark / clicked
/// highlight). The RGB we rasterise is therefore irrelevant; black keeps the
/// PNG readable if anything ever inspects it directly.
#[cfg(target_os = "macos")]
const ICON_COLOR_TEMPLATE: &str = "#000000";

/// Tooltip body used until the first [`set_status`] lands.
const DEFAULT_SUMMARY: &str = "Click to open Agent Usage Reporter";

/// User-driven actions that originate from the tray icon and end up
/// driving the GPUI side. We keep the enum small — "show the modal" or
/// "exit aura" — because that's the entirety of the wifi/volume tray
/// surface area we're imitating.
#[derive(Debug, Clone, Copy)]
pub enum TrayEvent {
    /// Primary-click on the icon (or middle-click, or "Show Aura" picked
    /// from the menu). `hint` carries the tray icon's screen coordinates
    /// when the host sent them with the activate request — `None` when the
    /// trigger was a menu item (which doesn't surface a click position).
    ///
    /// Plumbed end-to-end but unused on the consumer side for now: a
    /// first attempt at anchoring the modal next to the click on
    /// Wayland produced a malformed (very narrow) window for reasons
    /// we haven't root-caused yet, so the consumer falls back to its
    /// corner placement.
    Show {
        #[allow(dead_code)]
        hint: Option<(i32, i32)>,
    },
    /// "Quit Aura" picked from the right-click context menu — the user
    /// wants the process to actually exit (tray icon goes away,
    /// systemd's `Restart=on-failure` respects the clean exit).
    Quit,
}

/// Live indicator state. Pushed from whatever loaded fresh usage data (the
/// modal's refresh, or `main.rs`'s background poll) and applied to the icon on
/// the main thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrayStatus {
    /// One short line describing current usage, e.g. `"Claude · 5h 72%"`.
    /// Shown as the tooltip body on every platform.
    pub summary: String,
    /// Whether to emphasise the icon. Drives `NeedsAttention` +
    /// `AttentionIconPixmap` on StatusNotifierItem hosts and a red icon
    /// variant on macOS / Windows.
    pub attention: bool,
}

impl Default for TrayStatus {
    fn default() -> Self {
        Self {
            summary: DEFAULT_SUMMARY.to_string(),
            attention: false,
        }
    }
}

/// Status pushed by [`set_status`] but not yet applied to the icon.
///
/// The indirection exists because AppKit requires `NSStatusItem` mutation on
/// the main thread, and the callers that *have* fresh data (the refresh
/// worker, the background poll) run off it. `main.rs`'s poll loop drains this
/// on the GPUI main thread via [`apply_pending_status`].
static PENDING_STATUS: Mutex<Option<TrayStatus>> = Mutex::new(None);

/// Queue `status` for application to the tray icon. Safe to call from any
/// thread; a no-op if the tray failed to install.
pub fn set_status(status: TrayStatus) {
    if let Ok(mut pending) = PENDING_STATUS.lock() {
        *pending = Some(status);
    }
}

/// Opaque handle returned by [`install`]. Must be kept alive for the
/// lifetime of the app — dropping it removes the icon.
pub struct TrayHandle {
    #[cfg(target_os = "linux")]
    ksni: ksni::blocking::Handle<linux::AuraTray>,
    #[cfg(not(target_os = "linux"))]
    icon: tray_icon::TrayIcon,
    /// Last status actually pushed to the backend. Lets
    /// [`apply_pending_status`] skip redundant D-Bus / AppKit traffic when the
    /// poll produces the same numbers as last time.
    applied: TrayStatus,
}

impl TrayHandle {
    /// Apply any status queued by [`set_status`]. Call from the GPUI main
    /// thread only (AppKit requirement on macOS).
    pub fn apply_pending_status(&mut self) {
        let Some(next) = PENDING_STATUS.lock().ok().and_then(|mut p| p.take()) else {
            return;
        };
        if next == self.applied {
            return;
        }
        self.push_status(&next);
        self.applied = next;
    }

    #[cfg(target_os = "linux")]
    fn push_status(&mut self, status: &TrayStatus) {
        let status = status.clone();
        // `update` returns None once the service has shut down; nothing useful
        // to do about it here — the process is on its way out.
        let _ = self.ksni.update(move |tray: &mut linux::AuraTray| {
            tray.status = status;
        });
    }

    #[cfg(not(target_os = "linux"))]
    fn push_status(&mut self, status: &TrayStatus) {
        let _ = self.icon.set_tooltip(Some(non_linux::tooltip(status)));
        let Ok(icon) = non_linux::state_icon(status.attention) else {
            return;
        };
        // `set_icon` on macOS keeps the *previous* template flag, which would
        // leave the red attention icon getting alpha-recolored back to the
        // menu-bar foreground — i.e. invisible as a warning. The paired setter
        // swaps image and flag together. It is a no-op off macOS, hence the
        // split.
        #[cfg(target_os = "macos")]
        {
            let _ = self
                .icon
                .set_icon_with_as_template(Some(icon), non_linux::is_template(status.attention));
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = self.icon.set_icon(Some(icon));
        }
    }
}

/// Non-blocking poll. Returns the next pending [`TrayEvent`] or `None`.
/// Called from GPUI's async task on a short timer.
pub fn try_recv_event() -> Option<TrayEvent> {
    #[cfg(target_os = "linux")]
    {
        linux::try_recv()
    }
    #[cfg(not(target_os = "linux"))]
    {
        non_linux::try_recv()
    }
}

// ── Shared icon rasteriser ───────────────────────────────────────────────────
//
// Both backends paint the same brand SVG; only the destination buffer
// format differs (ksni wants ARGB32, tray-icon wants straight RGBA).

/// Rasterise the brand SVG at `size`×`size` in `color`.
///
/// Returns RGBA8 with *straight* (non-premultiplied) alpha — both call
/// sites massage it from here.
fn render_logo_rgba(size: u32, color: &str) -> Result<Vec<u8>> {
    let svg_text = std::str::from_utf8(AURA_LOGO_SVG).context("aura.svg is not UTF-8")?;
    let svg_text = svg_text.replace("currentColor", color);

    let tree =
        usvg::Tree::from_str(&svg_text, &usvg::Options::default()).context("parsing aura.svg")?;

    let mut pixmap = tiny_skia::Pixmap::new(size, size).context("allocating tray pixmap")?;

    let scale = size as f32 / tree.size().width().max(tree.size().height());
    let transform = tiny_skia::Transform::from_scale(scale, scale);

    resvg::render(&tree, transform, &mut pixmap.as_mut());

    // resvg produces premultiplied RGBA — demultiply to straight.
    let mut rgba = pixmap.take();
    for px in rgba.chunks_exact_mut(4) {
        let a = px[3];
        if a > 0 && a < 255 {
            let inv = 255.0 / a as f32;
            px[0] = (px[0] as f32 * inv).min(255.0) as u8;
            px[1] = (px[1] as f32 * inv).min(255.0) as u8;
            px[2] = (px[2] as f32 * inv).min(255.0) as u8;
        }
    }

    Ok(rgba)
}

// ── Linux: ksni ──────────────────────────────────────────────────────────────

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use ksni::blocking::TrayMethods;
    use ksni::{Icon, MenuItem, Status};
    use std::sync::mpsc::{self, Receiver, Sender};
    use std::sync::OnceLock;

    /// Channel used by `activate()` (primary-click), `secondary_activate()`
    /// (middle-click) and the fallback "Show Aura" menu item to signal the
    /// GPUI side. We stash the receiver in a process-global so
    /// `try_recv_event` can drain it without threading it through
    /// [`TrayHandle`].
    static EVENT_RX: OnceLock<Mutex<Receiver<TrayEvent>>> = OnceLock::new();

    pub(super) struct AuraTray {
        tx: Sender<TrayEvent>,
        icons: Vec<Icon>,
        attention_icons: Vec<Icon>,
        /// Freedesktop icon name, or empty when no themed `aura` icon is
        /// installed. See [`themed_icon_name`].
        icon_name: String,
        pub(super) status: TrayStatus,
    }

    impl ksni::Tray for AuraTray {
        fn id(&self) -> String {
            // Becomes part of the DBus object path; keep stable.
            "aura".into()
        }

        fn title(&self) -> String {
            "Aura — Agent Usage Reporter".into()
        }

        /// Prefer a themed icon when one is installed: the host can then
        /// render it at any panel size and follow the user's icon theme.
        /// Empty (→ hosts fall back to `icon_pixmap`) when the icon isn't on
        /// disk, which is the case for `cargo install` users who never ran
        /// `install.sh`.
        fn icon_name(&self) -> String {
            self.icon_name.clone()
        }

        fn icon_pixmap(&self) -> Vec<Icon> {
            self.icons.clone()
        }

        fn attention_icon_name(&self) -> String {
            // No separate themed asset ships for the attention state, so
            // always fall through to the pixmap below.
            String::new()
        }

        fn attention_icon_pixmap(&self) -> Vec<Icon> {
            self.attention_icons.clone()
        }

        /// `NeedsAttention` tells the host to emphasise the item (Plasma
        /// un-hides it from the overflow group and animates it) and to switch
        /// to `attention_icon_pixmap`.
        fn status(&self) -> Status {
            if self.status.attention {
                Status::NeedsAttention
            } else {
                Status::Active
            }
        }

        fn tool_tip(&self) -> ksni::ToolTip {
            ksni::ToolTip {
                title: "Aura".into(),
                description: self.status.summary.clone(),
                icon_name: String::new(),
                icon_pixmap: Vec::new(),
            }
        }

        /// Primary-click. Plasma + GNOME + most KSNI hosts route the
        /// user's left-click here, which is exactly the wifi-style UX
        /// we want. `x` / `y` are the icon's position in screen coords
        /// — we forward them so the modal can anchor near the icon.
        ///
        fn activate(&mut self, x: i32, y: i32) {
            let _ = self.tx.send(TrayEvent::Show { hint: Some((x, y)) });
        }

        /// Middle-click. The spec calls this "a secondary and less important
        /// form of activation"; for a single-surface app like Aura there is
        /// nothing secondary to do, so it opens the modal too. Doing nothing
        /// would read as a dead click.
        fn secondary_activate(&mut self, x: i32, y: i32) {
            self.activate(x, y);
        }

        /// Right-click → minimal context menu with the two explicit
        /// actions a tray indicator owes the user: "Show Aura" (same
        /// effect as left-clicking the icon) and "Quit Aura" (exit the
        /// process, tray icon goes away).
        fn menu(&self) -> Vec<MenuItem<Self>> {
            use ksni::menu::StandardItem;
            vec![
                StandardItem {
                    label: "Show Aura".into(),
                    activate: Box::new(|tray: &mut AuraTray| {
                        // Menu doesn't surface a click position — let
                        // the modal fall back to its corner placement.
                        let _ = tray.tx.send(TrayEvent::Show { hint: None });
                    }),
                    ..Default::default()
                }
                .into(),
                ksni::MenuItem::Separator,
                StandardItem {
                    label: "Quit Aura".into(),
                    activate: Box::new(|tray: &mut AuraTray| {
                        let _ = tray.tx.send(TrayEvent::Quit);
                    }),
                    ..Default::default()
                }
                .into(),
            ]
        }

        /// The panel went away (shell restart, extension toggled off). Return
        /// `true` to keep the service running so ksni re-registers when a
        /// watcher comes back — the alternative is a live process with no UI
        /// at all.
        fn watcher_offline(&self, reason: ksni::OfflineReason) -> bool {
            eprintln!(
                "aura: StatusNotifier host unavailable ({reason:?}); \
                 the tray icon will appear when one registers"
            );
            true
        }

        fn watcher_online(&self) {
            eprintln!("aura: StatusNotifier host is back; tray icon re-registered");
        }
    }

    /// Rasterise every entry in [`ICON_SIZES`] into ksni's ARGB32 layout.
    fn icon_set(color: &str) -> Result<Vec<Icon>> {
        ICON_SIZES
            .iter()
            .map(|&size| {
                // RGBA8 → ARGB32 (network byte order, big-endian).
                // ksni::Icon::data layout in memory is `[A, R, G, B, …]`.
                let mut argb = render_logo_rgba(size, color)?;
                for px in argb.chunks_exact_mut(4) {
                    px.rotate_right(1);
                }
                Ok(Icon {
                    width: size as i32,
                    height: size as i32,
                    data: argb,
                })
            })
            .collect()
    }

    /// `"aura"` when a themed icon is installed under any XDG data dir,
    /// otherwise the empty string.
    ///
    /// Reporting an `IconName` the theme can't resolve is worse than
    /// reporting none: hosts that trust the name over the pixmap render a
    /// blank slot. `install.sh` writes
    /// `~/.local/share/icons/hicolor/scalable/apps/aura.svg`, but a user who
    /// installed the binary by hand has no such file.
    fn themed_icon_name() -> String {
        let mut roots: Vec<std::path::PathBuf> = Vec::new();
        if let Some(home) = dirs::data_dir() {
            roots.push(home.join("icons"));
        }
        if let Some(home) = dirs::home_dir() {
            roots.push(home.join(".icons"));
        }
        let system_dirs = std::env::var("XDG_DATA_DIRS")
            .unwrap_or_else(|_| "/usr/local/share:/usr/share".to_string());
        roots.extend(
            system_dirs
                .split(':')
                .filter(|d| !d.is_empty())
                .map(|d| std::path::Path::new(d).join("icons")),
        );

        let candidates = [
            "hicolor/scalable/apps/aura.svg",
            "hicolor/symbolic/apps/aura-symbolic.svg",
            "hicolor/64x64/apps/aura.png",
            "hicolor/48x48/apps/aura.png",
        ];
        for root in roots {
            if candidates.iter().any(|c| root.join(c).exists()) {
                return "aura".to_string();
            }
        }
        String::new()
    }

    pub(super) fn install() -> Result<TrayHandle> {
        let (tx, rx) = mpsc::channel::<TrayEvent>();

        // Stash the receiver where `try_recv_event` can find it.
        // Re-install would overwrite — but install() is called once.
        let _ = EVENT_RX.set(Mutex::new(rx));

        let tray = AuraTray {
            tx,
            icons: icon_set(ICON_COLOR)?,
            attention_icons: icon_set(ICON_COLOR_ATTENTION)?,
            icon_name: themed_icon_name(),
            status: TrayStatus::default(),
        };

        // `assume_sni_available(true)` is load-bearing, not a nicety.
        //
        // With ksni's default (`false`), a missing
        // `org.kde.StatusNotifierWatcher` makes `spawn()` fail immediately
        // *and start no service*, so nothing ever retries. Aura would then sit
        // there as a live process with no icon and no window — invisible,
        // unkillable except from a task manager. Two ordinary situations hit
        // that: a session where the panel simply hasn't claimed the bus name
        // yet (our systemd unit is only ordered `After=graphical-session.target`,
        // which does not wait for the panel), and a desktop where SNI support
        // arrives later (GNOME's AppIndicator extension being enabled after
        // login).
        //
        // With it set, those cases route to `watcher_offline` instead and the
        // service keeps running, re-registering as soon as a host appears.
        let handle = tray
            .assume_sni_available(true)
            .spawn()
            .context("ksni spawn (register on D-Bus)")?;

        Ok(TrayHandle {
            ksni: handle,
            applied: TrayStatus::default(),
        })
    }

    pub(super) fn try_recv() -> Option<TrayEvent> {
        let mtx = EVENT_RX.get()?;
        mtx.lock().ok()?.try_recv().ok()
    }
}

#[cfg(target_os = "linux")]
pub fn install() -> Result<TrayHandle> {
    linux::install()
}

// ── macOS / Windows: tray-icon ───────────────────────────────────────────────

#[cfg(not(target_os = "linux"))]
mod non_linux {
    use super::*;
    use tray_icon::{
        menu::{Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem},
        Icon, MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent,
    };

    pub(super) const MENU_ID_SHOW: &str = "aura.show";
    pub(super) const MENU_ID_QUIT: &str = "aura.quit";

    /// AppKit draws the status item at 18 pt. Rasterising at the largest size
    /// we have keeps the backing store dense enough for a 2× menu bar and lets
    /// `NSImage::setSize` do the (high-quality) downscale.
    #[cfg(target_os = "macos")]
    pub(super) fn macos_icon_size() -> u32 {
        ICON_SIZES.iter().copied().max().unwrap_or(ICON_SIZE_MAX)
    }

    /// Shell_NotifyIcon does not resample nicely: it blits whatever `HICON` it
    /// is given into a `SM_CXSMICON` slot. Rasterising directly at that size —
    /// which already accounts for the system DPI — is visibly sharper than
    /// handing Windows a 64 px icon to squeeze into 16.
    #[cfg(target_os = "windows")]
    pub(super) fn windows_icon_size() -> u32 {
        use windows::Win32::UI::HiDpi::{GetDpiForSystem, GetSystemMetricsForDpi};
        use windows::Win32::UI::WindowsAndMessaging::SM_CXSMICON;

        let requested = unsafe {
            let dpi = GetDpiForSystem();
            GetSystemMetricsForDpi(SM_CXSMICON, dpi)
        };
        if requested <= 0 {
            return ICON_SIZE_MAX;
        }
        // Snap up to a size we rasterise, so odd DPI values (150% → 24) still
        // land on a clean render rather than a resample.
        ICON_SIZES
            .iter()
            .copied()
            .find(|&s| s >= requested as u32)
            .unwrap_or(ICON_SIZE_MAX)
    }

    fn icon_size() -> u32 {
        #[cfg(target_os = "macos")]
        {
            macos_icon_size()
        }
        #[cfg(target_os = "windows")]
        {
            windows_icon_size()
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        {
            ICON_SIZE_MAX
        }
    }

    /// Icon color for the current state.
    ///
    /// macOS gets a template image (alpha-only, recolored by AppKit) in the
    /// normal state so it matches every other menu-bar item in light mode,
    /// dark mode and while the item is click-highlighted. The attention state
    /// deliberately opts out of template rendering — the whole point is to be
    /// the one item in the bar that isn't the menu-bar's foreground color.
    fn icon_color(attention: bool) -> &'static str {
        if attention {
            return ICON_COLOR_ATTENTION;
        }
        #[cfg(target_os = "macos")]
        {
            ICON_COLOR_TEMPLATE
        }
        #[cfg(not(target_os = "macos"))]
        {
            ICON_COLOR
        }
    }

    /// Whether the icon for `attention` should be handed to AppKit as a
    /// template image. Always false off macOS (the flag is ignored there).
    pub(super) fn is_template(attention: bool) -> bool {
        cfg!(target_os = "macos") && !attention
    }

    pub(super) fn state_icon(attention: bool) -> Result<Icon> {
        let size = icon_size();
        let rgba = render_logo_rgba(size, icon_color(attention))?;
        Icon::from_rgba(rgba, size, size).context("Icon::from_rgba")
    }

    /// Tooltip text: the app name plus whatever the latest status says.
    pub(super) fn tooltip(status: &TrayStatus) -> String {
        format!("Aura — {}", status.summary)
    }

    pub(super) fn install() -> Result<TrayHandle> {
        let status = TrayStatus::default();

        let menu = Menu::new();
        let show = MenuItem::with_id(MenuId::new(MENU_ID_SHOW), "Show Aura", true, None);
        // Cmd+Q / Ctrl+Q is what users reach for to close a menu-bar app, and
        // an accelerator is the only way to offer it here: Aura has no
        // application menu bar to hang a standard Quit item off.
        let quit = MenuItem::with_id(MenuId::new(MENU_ID_QUIT), "Quit Aura", true, quit_accel());
        // Note: quitting removes the tray icon; the LaunchAgent will restart
        // aura automatically in ~5 seconds (KeepAlive + ThrottleInterval).
        menu.append(&show).context("menu append Show")?;
        menu.append(&PredefinedMenuItem::separator())
            .context("menu separator")?;
        menu.append(&quit).context("menu append Quit")?;

        let tray = TrayIconBuilder::new()
            .with_icon(state_icon(status.attention)?)
            // Template rendering makes the icon track the menu bar's
            // appearance (light / dark / highlighted) the way every native
            // status item does. macOS-only; ignored on Windows.
            .with_icon_as_template(is_template(status.attention))
            .with_tooltip(tooltip(&status))
            .with_menu(Box::new(menu))
            // Primary-click activates directly; menu is right-click only.
            .with_menu_on_left_click(false)
            .with_menu_on_right_click(true)
            .build()
            .context("building tray icon")?;

        Ok(TrayHandle {
            icon: tray,
            applied: status,
        })
    }

    fn quit_accel() -> Option<tray_icon::menu::accelerator::Accelerator> {
        // `CMD_OR_CTRL` is muda's own per-OS constant: Command on macOS,
        // Control elsewhere. The shortcut is live while the menu is open,
        // which is the only scope a context menu can claim — Aura is a
        // background app with no application menu bar to register a global
        // Quit against.
        use tray_icon::menu::accelerator::{Accelerator, Code, CMD_OR_CTRL};
        Some(Accelerator::new(Some(CMD_OR_CTRL), Code::KeyQ))
    }

    pub(super) fn try_recv() -> Option<TrayEvent> {
        // Menu first (right-click → "Show Aura" or "Quit Aura"). Drain the
        // whole queue rather than one item per poll, matching the icon-event
        // loop below: at a 150 ms cadence a burst of clicks would otherwise
        // dribble out one per tick.
        while let Ok(event) = MenuEvent::receiver().try_recv() {
            match event.id().0.as_str() {
                MENU_ID_SHOW => return Some(TrayEvent::Show { hint: None }),
                MENU_ID_QUIT => return Some(TrayEvent::Quit),
                _ => {}
            }
        }
        // Drain all pending icon events. We only act on a left-button
        // *release* (Up) so we fire exactly once per click and don't
        // toggle back off on the matching Down event.
        while let Ok(evt) = TrayIconEvent::receiver().try_recv() {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                position,
                ..
            } = evt
            {
                return Some(TrayEvent::Show {
                    hint: Some((position.x as i32, position.y as i32)),
                });
            }
        }
        None
    }
}

#[cfg(not(target_os = "linux"))]
pub fn install() -> Result<TrayHandle> {
    non_linux::install()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_declared_icon_size_rasterises() {
        for &size in ICON_SIZES {
            let rgba = render_logo_rgba(size, ICON_COLOR).expect("render");
            assert_eq!(rgba.len(), (size * size * 4) as usize);
        }
    }

    #[test]
    fn the_icon_sizes_are_sorted_smallest_first() {
        // `windows_icon_size` picks the first entry at or above the system
        // metric, which is only the *closest* one if the list is ordered.
        assert!(ICON_SIZES.windows(2).all(|w| w[0] < w[1]));
        assert_eq!(ICON_SIZES.iter().copied().max(), Some(ICON_SIZE_MAX));
    }

    #[test]
    fn the_attention_icon_differs_from_the_normal_one() {
        // A same-colored "attention" icon would make NeedsAttention invisible.
        let normal = render_logo_rgba(32, ICON_COLOR).expect("render");
        let attention = render_logo_rgba(32, ICON_COLOR_ATTENTION).expect("render");
        assert_ne!(normal, attention);
    }

    #[test]
    fn the_default_status_is_not_an_attention_state() {
        let status = TrayStatus::default();
        assert!(!status.attention);
        assert_eq!(status.summary, DEFAULT_SUMMARY);
    }
}
