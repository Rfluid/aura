use anyhow::Context as _;
use uuid::Uuid;
use x11rb::{connection::Connection as _, xcb_ffi::XCBConnection};

use crate::{Bounds, DisplayId, Pixels, PlatformDisplay, Size, point, px};

#[derive(Debug)]
pub(crate) struct X11Display {
    pub(super) x_screen_index: usize,
    id: DisplayId,
    bounds: Bounds<Pixels>,
    uuid: Uuid,
}

impl X11Display {
    pub(crate) fn new(
        xcb: &XCBConnection,
        scale_factor: f32,
        x_screen_index: usize,
    ) -> anyhow::Result<Self> {
        let screen = xcb
            .setup()
            .roots
            .get(x_screen_index)
            .with_context(|| format!("No screen found with index {x_screen_index}"))?;
        Ok(Self {
            x_screen_index,
            id: DisplayId(x_screen_index as u32),
            bounds: Bounds {
                origin: Default::default(),
                size: Size {
                    width: px(screen.width_in_pixels as f32 / scale_factor),
                    height: px(screen.height_in_pixels as f32 / scale_factor),
                },
            },
            uuid: Uuid::from_bytes([0; 16]),
        })
    }

    /// An X screen is the entire virtual desktop, including gaps between
    /// outputs. Use RandR monitors for placement, retaining the root index
    /// separately for visuals and GPU surface creation.
    pub(crate) fn for_screen(
        xcb: &XCBConnection,
        scale_factor: f32,
        x_screen_index: usize,
    ) -> Vec<Self> {
        use x11rb::protocol::randr::ConnectionExt as _;

        let monitors = (|| {
            let root = xcb.setup().roots.get(x_screen_index)?.root;
            let reply = xcb.randr_get_monitors(root, true).ok()?.reply().ok()?;
            let mut monitors = reply.monitors;
            monitors.sort_by_key(|monitor| !monitor.primary);
            Some(
                monitors
                    .into_iter()
                    .filter_map(|monitor| {
                        if monitor.width == 0 || monitor.height == 0 {
                            return None;
                        }
                        Some(Self {
                            x_screen_index,
                            id: DisplayId(monitor.name),
                            bounds: monitor_bounds(
                                monitor.x,
                                monitor.y,
                                monitor.width,
                                monitor.height,
                                scale_factor,
                            ),
                            uuid: Uuid::from_bytes([0; 16]),
                        })
                    })
                    .collect::<Vec<_>>(),
            )
        })()
        .unwrap_or_default();
        if monitors.is_empty() {
            Self::new(xcb, scale_factor, x_screen_index)
                .into_iter()
                .collect()
        } else {
            monitors
        }
    }
}

fn monitor_bounds(x: i16, y: i16, width: u16, height: u16, scale: f32) -> Bounds<Pixels> {
    Bounds {
        origin: point(px(x as f32 / scale), px(y as f32 / scale)),
        size: Size {
            width: px(width as f32 / scale),
            height: px(height as f32 / scale),
        },
    }
}

impl PlatformDisplay for X11Display {
    fn id(&self) -> DisplayId {
        self.id
    }

    fn uuid(&self) -> anyhow::Result<Uuid> {
        Ok(self.uuid)
    }

    fn bounds(&self) -> Bounds<Pixels> {
        self.bounds
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn monitors_exclude_the_gap_in_an_l_shaped_desktop() {
        let right = monitor_bounds(2880, 0, 4096, 1728, 1.6);
        let lower_left = monitor_bounds(0, 1440, 2880, 1800, 1.6);
        let left = monitor_bounds(320, 0, 2560, 1440, 1.6);
        let invisible_corner = point(px(3834.0), px(1677.0));
        assert!(
            ![right, lower_left, left]
                .iter()
                .any(|bounds| bounds.contains(&invisible_corner))
        );
        assert_eq!(right.origin, point(px(1800.0), px(0.0)));
        assert_eq!(
            right.size,
            Size {
                width: px(2560.0),
                height: px(1080.0)
            }
        );
        assert!(right.contains(&point(px(4210.0), px(1050.0))));
    }

    #[test]
    fn monitor_coordinates_preserve_negative_origins() {
        let left = monitor_bounds(-2560, -1440, 2560, 1440, 2.0);
        assert_eq!(left.origin, point(px(-1280.0), px(-720.0)));
        assert_eq!(left.bottom_right(), point(px(0.0), px(0.0)));
    }
}
