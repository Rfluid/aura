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
//! Both backends feed a unified [`TrayEvent`] stream that `lib.rs` drains
//! from the GPUI side via [`try_recv_event`], and both accept live state
//! pushes through [`set_status`] / [`apply_pending_status`] so the icon can
//! act as a real indicator instead of a static launcher.

use std::sync::Mutex;

use anyhow::{Context, Result};
use resvg::{tiny_skia, usvg};

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

/// Aura purple — must stay in sync with `aura-core/src/theme_default.toml` so
/// the tray icon matches the in-app brand color. The resting color, and the
/// bottom of the usage ramp.
const ICON_COLOR: &str = "#8b5cf6";

/// Usage ramp above [`ELEVATED_PERCENT`] / [`HIGH_PERCENT`] /
/// [`ATTENTION_PERCENT`]. Purple → yellow → orange → red is the ordering a
/// glance already knows how to read, and each step is a sibling of
/// [`ICON_COLOR`] in the same palette family so the mark still looks like
/// Aura's at every level.
const ICON_COLOR_ELEVATED: &str = "#eab308";
const ICON_COLOR_HIGH: &str = "#f97316";
/// Top of the ramp — also the danger color used by the modal's progress bars.
const ICON_COLOR_CRITICAL: &str = "#ef4444";

/// Usage at which the icon turns red and the item flips into its attention
/// state. Deliberately high: the whole value of `NeedsAttention` is that it
/// stays rare, and Plasma un-hides the icon from the overflow group when it
/// fires.
pub const ATTENTION_PERCENT: u8 = 90;

/// Ramp steps below [`ATTENTION_PERCENT`]. Neither is an alarm — they are
/// there so the color has already started moving by the time the arc is worth
/// looking at.
const HIGH_PERCENT: u8 = 75;
const ELEVATED_PERCENT: u8 = 50;

/// macOS template images are drawn from their alpha channel alone — AppKit
/// recolors them for the current menu-bar appearance (light / dark / clicked
/// highlight). The RGB we rasterise is therefore irrelevant; black keeps the
/// PNG readable if anything ever inspects it directly.
#[cfg(target_os = "macos")]
const ICON_COLOR_TEMPLATE: &str = "#000000";

/// Tooltip body used until the first [`set_status`] lands.
const DEFAULT_SUMMARY: &str = "Click to open Agent Usage Reporter";

/// Where the tray icon sits on screen, in **logical** pixels with the origin
/// at the top-left of the virtual desktop — the same space
/// `gpui::App::displays()` reports bounds in.
///
/// Every backend hands us physical (device) pixels; the conversion to logical
/// happens at the edge, in [`try_recv_event`], so nothing downstream has to
/// think about HiDPI. Getting that wrong is not a cosmetic bug: on a 2× Retina
/// display an unconverted X is double the real one, which pushes the modal off
/// the right edge of the screen and makes the clamp in
/// `placement::modal_origin` pin it to the corner on every single click.
#[derive(Debug, Clone, Copy)]
pub struct TrayAnchor {
    /// The click position.
    pub point: (f32, f32),
    /// The icon's own rect (`x`, `y`, `width`, `height`) when the host
    /// reported one. Preferred over [`Self::point`] for anchoring: a popover
    /// belongs under the icon, not under wherever the pointer happened to be.
    /// `None` on Linux — StatusNotifierItem's `Activate` carries a position
    /// hint but no geometry.
    pub rect: Option<(f32, f32, f32, f32)>,
}

impl TrayAnchor {
    /// Horizontal center to align the modal against: the middle of the icon
    /// when we know its rect, else the click X.
    ///
    /// macOS and Linux both anchor horizontally to the icon (see
    /// `placement::modal_origin`); Windows right-aligns to the screen edge
    /// instead and never calls this.
    #[cfg_attr(target_os = "windows", allow(dead_code))]
    pub fn center_x(&self) -> f32 {
        match self.rect {
            Some((x, _, w, _)) => x + w / 2.0,
            None => self.point.0,
        }
    }

    /// A point that is guaranteed to be on the display the icon lives on.
    /// Used to pick which monitor the modal opens on.
    pub fn locator(&self) -> (f32, f32) {
        match self.rect {
            Some((x, y, w, h)) => (x + w / 2.0, y + h / 2.0),
            None => self.point,
        }
    }
}

/// User-driven actions that originate from the tray icon and end up
/// driving the GPUI side. We keep the enum small — "show the modal" or
/// "exit aura" — because that's the entirety of the wifi/volume tray
/// surface area we're imitating.
#[derive(Debug, Clone, Copy)]
pub enum TrayEvent {
    /// Primary-click on the icon (or middle-click, or "Show Aura" picked
    /// from the menu). `anchor` carries the icon's screen geometry when the
    /// host sent it — `None` when the trigger was a menu item, which doesn't
    /// surface a position.
    Show { anchor: Option<TrayAnchor> },
    /// "Open config file" picked from the right-click context menu.
    OpenConfig,
    /// "Configuration guide" picked from the right-click context menu.
    OpenConfigTutorial,
    /// "Quit Aura" picked from the right-click context menu — the user
    /// wants the process to actually exit (tray icon goes away,
    /// systemd's `Restart=on-failure` respects the clean exit).
    Quit,
}

/// Which parts of the indicator are switched on, mirroring the
/// `tray.*` config keys.
///
/// Carried on [`TrayStatus`] rather than read from config down here so the
/// rendering path stays a pure function of the status it is handed — the same
/// reason the backends never look at `AppConfig` themselves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrayVisuals {
    /// Fill the ring in proportion to usage. Off → the ring is drawn whole.
    pub progress: bool,
    /// Climb the color ramp as usage does. Off → always Aura purple.
    pub color: bool,
    /// Ask the desktop to emphasise the item at [`ATTENTION_PERCENT`].
    pub pulse: bool,
}

impl Default for TrayVisuals {
    /// Progress and color on, pulse off. Drawing our own icon differently is
    /// ours to decide; making the desktop shout is not — see
    /// `TrayConfig::pulse`.
    fn default() -> Self {
        Self {
            progress: true,
            color: true,
            pulse: false,
        }
    }
}

/// Live indicator state. Pushed from whatever loaded fresh usage data (the
/// modal's refresh, or `lib.rs`'s background poll) and applied to the icon on
/// the main thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrayStatus {
    /// One short line describing current usage, e.g. `"Claude · 5h 72%"`.
    /// Shown as the tooltip body on every platform.
    pub summary: String,
    /// Usage that fills the ring, in whole percent clamped to `0..=100`.
    ///
    /// `None` means "no reading yet" — the icon then draws the complete ring
    /// at full opacity rather than an empty gauge, which would read as 0% and
    /// be a lie.
    ///
    /// Whole percent rather than the `f64` it is derived from for two
    /// reasons: it keeps this type `Eq`, and it quantises the diff in
    /// [`TrayHandle::apply_pending_status`], so a poll that moves usage by a
    /// fraction of a percent — well under a pixel of arc at 16 px — costs no
    /// D-Bus or AppKit traffic.
    pub gauge_percent: Option<u8>,
    /// Usage that drives the color ramp and the attention request. Same units
    /// and same `None` meaning as [`Self::gauge_percent`].
    ///
    /// A separate reading because the two halves of the indicator read
    /// different quota windows — by default the session for the ring and the
    /// week for the color, repointed per agent with `tray_progress_source` /
    /// `tray_color_source`.
    pub color_percent: Option<u8>,
    /// Which of the three visuals the user has left switched on.
    pub visuals: TrayVisuals,
}

impl Default for TrayStatus {
    fn default() -> Self {
        Self {
            summary: DEFAULT_SUMMARY.to_string(),
            gauge_percent: None,
            color_percent: None,
            visuals: TrayVisuals::default(),
        }
    }
}

impl TrayStatus {
    /// Whether to ask the desktop to emphasise the icon. Drives
    /// `NeedsAttention` + `AttentionIconPixmap` on StatusNotifierItem hosts.
    /// Always false unless the user opted into `tray.pulse`.
    pub fn attention(&self) -> bool {
        self.visuals.pulse
            && self
                .color_percent
                .is_some_and(|pct| pct >= ATTENTION_PERCENT)
    }

    /// How far to fill the ring, or `None` to draw it whole — which is both
    /// "no reading yet" and "the user turned the gauge off".
    fn gauge_usage(&self) -> Option<u8> {
        self.visuals
            .progress
            .then_some(self.gauge_percent)
            .flatten()
    }

    /// Color for the ring and the dot at this usage level.
    fn color(&self) -> &'static str {
        if !self.visuals.color {
            return ICON_COLOR;
        }
        match self.color_percent {
            Some(pct) if pct >= ATTENTION_PERCENT => ICON_COLOR_CRITICAL,
            Some(pct) if pct >= HIGH_PERCENT => ICON_COLOR_HIGH,
            Some(pct) if pct >= ELEVATED_PERCENT => ICON_COLOR_ELEVATED,
            _ => ICON_COLOR,
        }
    }

    /// Whether the rendered mark carries no usage signal at all: no reading
    /// yet, or both drawn visuals switched off. Only macOS cares — that is
    /// exactly the case where the icon is the static tray mark and belongs in
    /// template rendering.
    #[cfg_attr(target_os = "linux", allow(dead_code))]
    fn is_static(&self) -> bool {
        self.gauge_usage().is_none() && self.color() == ICON_COLOR
    }
}

/// Status pushed by [`set_status`] but not yet applied to the icon.
///
/// The indirection exists because AppKit requires `NSStatusItem` mutation on
/// the main thread, and the callers that *have* fresh data (the refresh
/// worker, the background poll) run off it. `lib.rs`'s poll loop drains this
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
        // Rasterise out here rather than inside the closure: hosts read
        // `IconPixmap` whenever they feel like it, and re-rendering six SVGs
        // per read would put the rasteriser on the D-Bus reply path. A render
        // failure leaves the previous icons in place — a stale gauge beats a
        // blank slot — while the tooltip still updates.
        let icons = linux::icon_set(status).ok();
        let status = status.clone();
        // `update` returns None once the service has shut down; nothing useful
        // to do about it here — the process is on its way out.
        let _ = self.ksni.update(move |tray: &mut linux::AuraTray| {
            tray.status = status;
            if let Some(icons) = icons {
                tray.icons = icons;
            }
        });
    }

    #[cfg(not(target_os = "linux"))]
    fn push_status(&mut self, status: &TrayStatus) {
        let _ = self.icon.set_tooltip(Some(non_linux::tooltip(status)));
        let Ok(icon) = non_linux::state_icon(status) else {
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
                .set_icon_with_as_template(Some(icon), non_linux::is_template(status));
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

// ── The gauge ────────────────────────────────────────────────────────────────
//
// The mark keeps Aura's center dot, but completes the surrounding ring for the
// tray: users read a full circumference as progress more readily than the
// brand logo's open arc. The ring fills from twelve o'clock and changes color
// as usage climbs. It is generated here rather than read from the static asset
// because it has to be split into consumed and remaining portions at runtime.

/// Side of the square canvas the mark is drawn on, and its centre.
const GAUGE_SIZE: f32 = 32.0;
const GAUGE_CENTER: f32 = GAUGE_SIZE / 2.0;
const GAUGE_RADIUS: f32 = 12.0;
const GAUGE_STROKE: f32 = 2.0;
const GAUGE_DOT_RADIUS: f32 = 3.0;

/// Where the ring starts, in SVG degrees — 0° is `+x` and angles grow
/// clockwise on screen, so 270° is straight up. The gauge therefore fills
/// from twelve o'clock, the one position on a dial that needs no explaining.
const GAUGE_START_DEG: f32 = 270.0;

/// A tray progress indicator uses the complete circumference even though the
/// Aura brand mark has an open ring.
const GAUGE_SWEEP_DEG: f32 = 360.0;

/// Opacity of the not-yet-consumed remainder of the ring. Low enough to read
/// as a track sitting behind the gauge, high enough to survive a 16 px raster
/// on a panel that may be any color.
const GAUGE_TRACK_OPACITY: f32 = 0.3;

/// Point on the ring at `degrees`.
fn ring_point(degrees: f32) -> (f32, f32) {
    let (sin, cos) = degrees.to_radians().sin_cos();
    (
        GAUGE_CENTER + GAUGE_RADIUS * cos,
        GAUGE_CENTER + GAUGE_RADIUS * sin,
    )
}

/// SVG path for `sweep` degrees of the ring, starting at [`GAUGE_START_DEG`]
/// and running counter-clockwise on screen, consistent with the brand mark's
/// arc direction.
fn ring_arc(sweep: f32) -> String {
    let (x0, y0) = ring_point(GAUGE_START_DEG);
    // SVG treats an arc whose start and end points coincide as empty. Split
    // the complete circumference into two semicircles so both the track and
    // the 100% fill remain visible.
    if sweep >= 360.0 {
        let (xm, ym) = ring_point(GAUGE_START_DEG - 180.0);
        return format!(
            "M {x0:.3} {y0:.3} A {r:.3} {r:.3} 0 0 0 {xm:.3} {ym:.3} A {r:.3} {r:.3} 0 0 0 {x0:.3} {y0:.3}",
            r = GAUGE_RADIUS
        );
    }
    let (x1, y1) = ring_point(GAUGE_START_DEG - sweep);
    // The arc flag picks the long way round whenever the sweep is a reflex
    // angle; without it every arc past 180° would be drawn as its short
    // complement, i.e. the gauge would collapse instead of filling.
    let large = u8::from(sweep > 180.0);
    format!(
        "M {x0:.3} {y0:.3} A {r:.3} {r:.3} 0 {large} 0 {x1:.3} {y1:.3}",
        r = GAUGE_RADIUS
    )
}

/// The whole mark for `usage`, drawn in `color`.
fn gauge_svg(usage: Option<u8>, color: &str) -> String {
    let track = ring_arc(GAUGE_SWEEP_DEG);
    let ring = match usage {
        // Nothing measured yet: a full-opacity ring with no fill in front of
        // it is the honest picture of "no reading" rather than 0%.
        None => format!(r#"<path d="{track}"/>"#),
        Some(pct) => {
            let sweep = GAUGE_SWEEP_DEG * f32::from(pct.min(100)) / 100.0;
            let mut ring = format!(r#"<path d="{track}" stroke-opacity="{GAUGE_TRACK_OPACITY}"/>"#);
            // Skipped at 0%: a round cap on a zero-length arc still paints a
            // full-width blob at twelve o'clock, which reads as usage.
            if sweep > 0.0 {
                ring.push_str(&format!(r#"<path d="{}"/>"#, ring_arc(sweep)));
            }
            ring
        }
    };

    format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {GAUGE_SIZE} {GAUGE_SIZE}" fill="none" stroke="{color}" stroke-width="{GAUGE_STROKE}" stroke-linecap="round">{ring}<circle cx="{GAUGE_CENTER}" cy="{GAUGE_CENTER}" r="{GAUGE_DOT_RADIUS}" fill="{color}" stroke="none"/></svg>"#
    )
}

// ── Shared icon rasteriser ───────────────────────────────────────────────────
//
// Both backends paint the same gauge; only the destination buffer format
// differs (ksni wants ARGB32, tray-icon wants straight RGBA).

/// Rasterise the mark for `usage` at `size`×`size` in `color`.
///
/// Returns RGBA8 with *straight* (non-premultiplied) alpha — both call
/// sites massage it from here.
fn render_gauge_rgba(size: u32, usage: Option<u8>, color: &str) -> Result<Vec<u8>> {
    let svg_text = gauge_svg(usage, color);

    let tree = usvg::Tree::from_str(&svg_text, &usvg::Options::default())
        .context("parsing the tray gauge SVG")?;

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
        /// The gauge rendered for [`Self::status`], one entry per
        /// [`ICON_SIZES`]. Recomputed by `TrayHandle::push_status` whenever
        /// the status changes.
        pub(super) icons: Vec<Icon>,
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

        /// Always empty, so hosts use [`Self::icon_pixmap`].
        ///
        /// A themed `IconName` used to be reported when `install.sh` had put
        /// `aura.svg` in an XDG icon dir, and hosts prefer the name over the
        /// pixmap — which now means they would render the static logo and
        /// never show the gauge. A file on disk cannot track live usage, so
        /// there is nothing to name.
        fn icon_name(&self) -> String {
            String::new()
        }

        fn icon_pixmap(&self) -> Vec<Icon> {
            self.icons.clone()
        }

        fn attention_icon_name(&self) -> String {
            String::new()
        }

        /// Same pixmaps as [`Self::icon_pixmap`]: the ramp has already turned
        /// the gauge red by the time the item reports `NeedsAttention`, and
        /// hosts switch to this property when it does. Rendering a second set
        /// would only risk the two disagreeing.
        fn attention_icon_pixmap(&self) -> Vec<Icon> {
            self.icons.clone()
        }

        /// `NeedsAttention` tells the host to emphasise the item (Plasma
        /// un-hides it from the overflow group and animates it) and to switch
        /// to `attention_icon_pixmap`.
        fn status(&self) -> Status {
            if self.status.attention() {
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
        /// Note the units: unlike the macOS / Windows backends, SNI hosts
        /// report the hint in the same logical pixel space the compositor
        /// uses for window geometry, so no scale conversion is applied here.
        fn activate(&mut self, x: i32, y: i32) {
            let _ = self.tx.send(TrayEvent::Show {
                anchor: Some(TrayAnchor {
                    point: (x as f32, y as f32),
                    rect: None,
                }),
            });
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
                        let _ = tray.tx.send(TrayEvent::Show { anchor: None });
                    }),
                    ..Default::default()
                }
                .into(),
                StandardItem {
                    label: "Open config file".into(),
                    activate: Box::new(|tray: &mut AuraTray| {
                        let _ = tray.tx.send(TrayEvent::OpenConfig);
                    }),
                    ..Default::default()
                }
                .into(),
                StandardItem {
                    label: "Configuration guide".into(),
                    activate: Box::new(|tray: &mut AuraTray| {
                        let _ = tray.tx.send(TrayEvent::OpenConfigTutorial);
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

    /// Rasterise the gauge for `status` at every entry in [`ICON_SIZES`], in
    /// ksni's ARGB32 layout.
    pub(super) fn icon_set(status: &TrayStatus) -> Result<Vec<Icon>> {
        let color = status.color();
        let usage = status.gauge_usage();
        ICON_SIZES
            .iter()
            .map(|&size| {
                // RGBA8 → ARGB32 (network byte order, big-endian).
                // ksni::Icon::data layout in memory is `[A, R, G, B, …]`.
                let mut argb = render_gauge_rgba(size, usage, color)?;
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

    pub(super) fn install() -> Result<TrayHandle> {
        let (tx, rx) = mpsc::channel::<TrayEvent>();

        // Stash the receiver where `try_recv_event` can find it.
        // Re-install would overwrite — but install() is called once.
        let _ = EVENT_RX.set(Mutex::new(rx));

        let status = TrayStatus::default();
        let tray = AuraTray {
            tx,
            icons: icon_set(&status)?,
            status,
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
    pub(super) const MENU_ID_OPEN_CONFIG: &str = "aura.open-config";
    pub(super) const MENU_ID_CONFIG_TUTORIAL: &str = "aura.config-tutorial";
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
    /// While the mark carries no usage signal — no reading yet, or the drawn
    /// visuals switched off — macOS gets a template image (alpha-only,
    /// recolored by AppKit) so the static mark matches every other menu-bar
    /// item in light mode, dark mode and while the item is click-highlighted;
    /// the RGB we rasterise is then irrelevant. Once there is something to
    /// show, the color *is* the signal, so template rendering is off and the
    /// ramp shows through — an alpha-recolored gauge would be the one thing it
    /// must not be, the menu bar's foreground color.
    fn icon_color(status: &TrayStatus) -> &'static str {
        #[cfg(target_os = "macos")]
        {
            if status.is_static() {
                return ICON_COLOR_TEMPLATE;
            }
        }
        status.color()
    }

    /// Whether `status`'s icon should be handed to AppKit as a template
    /// image. Always false off macOS (the flag is ignored there).
    pub(super) fn is_template(status: &TrayStatus) -> bool {
        cfg!(target_os = "macos") && status.is_static()
    }

    pub(super) fn state_icon(status: &TrayStatus) -> Result<Icon> {
        let size = icon_size();
        let rgba = render_gauge_rgba(size, status.gauge_usage(), icon_color(status))?;
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
        let open_config = MenuItem::with_id(
            MenuId::new(MENU_ID_OPEN_CONFIG),
            "Open config file",
            true,
            None,
        );
        let config_tutorial = MenuItem::with_id(
            MenuId::new(MENU_ID_CONFIG_TUTORIAL),
            "Configuration guide",
            true,
            None,
        );
        // Cmd+Q / Ctrl+Q is what users reach for to close a menu-bar app, and
        // an accelerator is the only way to offer it here: Aura has no
        // application menu bar to hang a standard Quit item off.
        let quit = MenuItem::with_id(MenuId::new(MENU_ID_QUIT), "Quit Aura", true, quit_accel());
        // Note: quitting removes the tray icon; the LaunchAgent will restart
        // aura automatically in ~5 seconds (KeepAlive + ThrottleInterval).
        menu.append(&show).context("menu append Show")?;
        menu.append(&open_config)
            .context("menu append Open config file")?;
        menu.append(&config_tutorial)
            .context("menu append Configuration guide")?;
        menu.append(&PredefinedMenuItem::separator())
            .context("menu separator")?;
        menu.append(&quit).context("menu append Quit")?;

        let tray = TrayIconBuilder::new()
            .with_icon(state_icon(&status)?)
            // Template rendering makes the icon track the menu bar's
            // appearance (light / dark / highlighted) the way every native
            // status item does. macOS-only; ignored on Windows.
            .with_icon_as_template(is_template(&status))
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

    /// Convert a physical (device-pixel) screen point reported by tray-icon
    /// into the logical space `TrayAnchor` is documented in.
    fn to_logical(x: f64, y: f64, scale: f32) -> (f32, f32) {
        (x as f32 / scale, y as f32 / scale)
    }

    pub(super) fn try_recv() -> Option<TrayEvent> {
        // Menu first (right-click → "Show Aura" or "Quit Aura"). Drain the
        // whole queue rather than one item per poll, matching the icon-event
        // loop below: at a 150 ms cadence a burst of clicks would otherwise
        // dribble out one per tick.
        while let Ok(event) = MenuEvent::receiver().try_recv() {
            match event.id().0.as_str() {
                MENU_ID_SHOW => return Some(TrayEvent::Show { anchor: None }),
                MENU_ID_OPEN_CONFIG => return Some(TrayEvent::OpenConfig),
                MENU_ID_CONFIG_TUTORIAL => return Some(TrayEvent::OpenConfigTutorial),
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
                rect,
                ..
            } = evt
            {
                // Both backends report physical pixels: macOS multiplies by
                // the status bar window's `backingScaleFactor`, Windows uses
                // raw `GetCursorPos` / `Shell_NotifyIconGetRect` output.
                let scale = crate::platform::tray_scale_factor(position.x, position.y);
                let point = to_logical(position.x, position.y, scale);
                let (rx, ry) = to_logical(rect.position.x, rect.position.y, scale);
                let (rw, rh) = (
                    rect.size.width as f32 / scale,
                    rect.size.height as f32 / scale,
                );
                // A zero-area rect means the host had no geometry for us;
                // fall back to the click point rather than anchoring to a
                // degenerate box at the origin.
                let rect = (rw > 0.0 && rh > 0.0).then_some((rx, ry, rw, rh));
                return Some(TrayEvent::Show {
                    anchor: Some(TrayAnchor { point, rect }),
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
    fn the_icon_rect_wins_over_the_click_point() {
        // Anchoring to the icon rather than the pointer is what keeps the
        // popover centred under the icon no matter where inside it the user
        // clicked.
        let anchor = TrayAnchor {
            point: (1012.0, 11.0),
            rect: Some((1000.0, 0.0, 24.0, 24.0)),
        };
        assert_eq!(anchor.center_x(), 1012.0);
        assert_eq!(anchor.locator(), (1012.0, 12.0));
    }

    #[test]
    fn without_a_rect_both_helpers_fall_back_to_the_click() {
        // The StatusNotifierItem `Activate` hint carries a position but no
        // geometry, so Linux always lands here.
        let anchor = TrayAnchor {
            point: (1900.0, 1040.0),
            rect: None,
        };
        assert_eq!(anchor.center_x(), 1900.0);
        assert_eq!(anchor.locator(), (1900.0, 1040.0));
    }

    /// A status with every visual on, so the ramp and gauge tests exercise
    /// the drawing itself rather than the toggles.
    fn status(usage_percent: Option<u8>) -> TrayStatus {
        TrayStatus {
            summary: String::new(),
            gauge_percent: usage_percent,
            color_percent: usage_percent,
            visuals: TrayVisuals {
                progress: true,
                color: true,
                pulse: true,
            },
        }
    }

    #[test]
    fn every_declared_icon_size_rasterises() {
        for &size in ICON_SIZES {
            let rgba = render_gauge_rgba(size, Some(50), ICON_COLOR).expect("render");
            assert_eq!(rgba.len(), (size * size * 4) as usize);
        }
    }

    #[test]
    fn the_attention_icon_differs_from_the_normal_one() {
        // A same-colored "attention" icon would make NeedsAttention invisible.
        let normal = render_gauge_rgba(32, Some(10), ICON_COLOR).expect("render");
        let attention = render_gauge_rgba(32, Some(95), ICON_COLOR_CRITICAL).expect("render");
        assert_ne!(normal, attention);
    }

    #[test]
    fn the_default_status_is_not_an_attention_state() {
        let status = TrayStatus::default();
        assert!(!status.attention());
        assert_eq!(status.gauge_percent, None);
        assert_eq!(status.color_percent, None);
        assert_eq!(status.summary, DEFAULT_SUMMARY);
    }

    #[test]
    fn the_color_ramp_climbs_purple_yellow_orange_red() {
        assert_eq!(status(None).color(), ICON_COLOR);
        assert_eq!(status(Some(0)).color(), ICON_COLOR);
        assert_eq!(status(Some(ELEVATED_PERCENT - 1)).color(), ICON_COLOR);
        assert_eq!(status(Some(ELEVATED_PERCENT)).color(), ICON_COLOR_ELEVATED);
        assert_eq!(status(Some(HIGH_PERCENT - 1)).color(), ICON_COLOR_ELEVATED);
        assert_eq!(status(Some(HIGH_PERCENT)).color(), ICON_COLOR_HIGH);
        assert_eq!(status(Some(ATTENTION_PERCENT - 1)).color(), ICON_COLOR_HIGH);
        assert_eq!(status(Some(ATTENTION_PERCENT)).color(), ICON_COLOR_CRITICAL);
        assert_eq!(status(Some(100)).color(), ICON_COLOR_CRITICAL);
    }

    #[test]
    fn attention_starts_exactly_at_the_threshold() {
        assert!(!status(None).attention());
        assert!(!status(Some(ATTENTION_PERCENT - 1)).attention());
        assert!(status(Some(ATTENTION_PERCENT)).attention());
    }

    #[test]
    fn the_arc_starts_at_the_top_and_fills_anticlockwise() {
        // Twelve o'clock on a 32×32 canvas with r=12.
        assert_eq!(ring_point(GAUGE_START_DEG), (16.0, 4.0));
        // A quarter of the full ring ends at nine o'clock.
        let (x, y) = ring_point(GAUGE_START_DEG - GAUGE_SWEEP_DEG / 4.0);
        assert!((x - 4.0).abs() < 0.01, "x was {x}");
        assert!((y - 16.0).abs() < 0.01, "y was {y}");
    }

    #[test]
    fn a_full_gauge_returns_to_twelve_oclock() {
        let full = ring_point(GAUGE_START_DEG - GAUGE_SWEEP_DEG);
        assert!((full.0 - 16.0).abs() < 0.01, "x was {}", full.0);
        assert!((full.1 - 4.0).abs() < 0.01, "y was {}", full.1);
    }

    #[test]
    fn the_large_arc_flag_tracks_the_sweep() {
        // Past a half-turn the short complement would be drawn instead,
        // collapsing the gauge just as it gets interesting.
        assert!(ring_arc(90.0).contains(" 0 0 "));
        assert!(ring_arc(270.0).contains(" 1 0 "));
    }

    #[test]
    fn a_complete_circle_is_split_into_two_visible_arcs() {
        // A single 360° SVG arc has coincident endpoints and renders empty.
        assert_eq!(ring_arc(GAUGE_SWEEP_DEG).matches(" A ").count(), 2);
        assert_eq!(gauge_svg(Some(100), ICON_COLOR).matches(" A ").count(), 4);
        let empty = render_gauge_rgba(16, Some(0), ICON_COLOR).expect("render empty gauge");
        let full = render_gauge_rgba(16, Some(100), ICON_COLOR).expect("render full gauge");
        assert_ne!(empty, full);
    }

    #[test]
    fn an_unread_gauge_is_the_static_full_ring() {
        // One full-opacity circle, no track and no fill.
        let svg = gauge_svg(None, ICON_COLOR);
        assert_eq!(svg.matches("<path").count(), 1);
        assert_eq!(svg.matches(" A ").count(), 2);
        assert!(!svg.contains("stroke-opacity"));
    }

    #[test]
    fn a_measured_gauge_draws_a_track_behind_the_fill() {
        let svg = gauge_svg(Some(40), ICON_COLOR);
        assert_eq!(svg.matches("<path").count(), 2);
        assert!(svg.contains("stroke-opacity"));
    }

    #[test]
    fn zero_percent_draws_no_fill_at_all() {
        // A round cap on a zero-length arc paints a blob that reads as usage.
        let svg = gauge_svg(Some(0), ICON_COLOR);
        assert_eq!(svg.matches("<path").count(), 1);
        assert!(svg.contains("stroke-opacity"));
    }

    #[test]
    fn usage_past_a_hundred_percent_does_not_overrun_the_ring() {
        // Quota APIs have been known to report >100; the arc must saturate
        // rather than wrap back over itself.
        assert_eq!(
            gauge_svg(Some(100), ICON_COLOR),
            gauge_svg(Some(250), ICON_COLOR)
        );
    }

    #[test]
    fn pulse_off_keeps_the_desktop_quiet_but_still_paints_the_gauge() {
        // The shipped default. Everything Aura draws itself still reacts;
        // only the request for the host to emphasise the item is withheld.
        let mut status = status(Some(95));
        status.visuals.pulse = false;
        assert!(!status.attention());
        assert_eq!(status.color(), ICON_COLOR_CRITICAL);
        assert_eq!(status.gauge_usage(), Some(95));
    }

    #[test]
    fn progress_off_draws_the_ring_whole_but_keeps_the_color() {
        let mut status = status(Some(95));
        status.visuals.progress = false;
        assert_eq!(status.gauge_usage(), None);
        assert_eq!(status.color(), ICON_COLOR_CRITICAL);
        assert!(!status.is_static());
    }

    #[test]
    fn color_off_pins_the_icon_to_aura_purple() {
        let mut status = status(Some(95));
        status.visuals.color = false;
        assert_eq!(status.color(), ICON_COLOR);
        // The gauge is the remaining signal, so the mark is not static.
        assert_eq!(status.gauge_usage(), Some(95));
        assert!(!status.is_static());
    }

    #[test]
    fn both_drawn_visuals_off_is_the_static_mark() {
        // Nothing left to say with the icon — on macOS this is what puts it
        // back into template rendering alongside every other status item.
        let mut status = status(Some(95));
        status.visuals.progress = false;
        status.visuals.color = false;
        assert!(status.is_static());
        assert_eq!(
            gauge_svg(status.gauge_usage(), status.color()),
            gauge_svg(None, ICON_COLOR)
        );
    }

    #[test]
    fn the_toggles_are_part_of_the_diff() {
        // `apply_pending_status` skips a push when the status is unchanged;
        // a config edit that only flips a visual still has to get through.
        let mut flipped = status(Some(40));
        flipped.visuals.progress = false;
        assert_ne!(status(Some(40)), flipped);
    }

    #[test]
    fn different_usage_levels_rasterise_differently() {
        // The whole feature: the icon has to actually change as usage climbs,
        // at the smallest size a panel will ask for.
        let low = render_gauge_rgba(16, Some(10), ICON_COLOR).expect("render");
        let high = render_gauge_rgba(16, Some(80), ICON_COLOR).expect("render");
        assert_ne!(low, high);
    }
}
