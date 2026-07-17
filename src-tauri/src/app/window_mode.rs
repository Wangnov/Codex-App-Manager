//! Window form factor. The manager is a compact 400×640 popover by default
//! (the at-a-glance dashboard) and expands into a desktop-sized workbench for
//! space-hungry surfaces (theme gallery, future config editors). One window,
//! two shapes: compact is fixed-size, expanded is user-resizable within a
//! minimum, and both transitions keep the window's visual center in place,
//! clamped into the current monitor's work area so no edge ends up off-screen.

use serde::{Deserialize, Serialize};
use tauri::{LogicalSize, Manager, PhysicalPosition};

use crate::errors::AppError;

pub const COMPACT_SIZE: (f64, f64) = (400.0, 640.0);
pub const EXPANDED_DEFAULT_SIZE: (f64, f64) = (1100.0, 720.0);
pub const EXPANDED_MIN_SIZE: (f64, f64) = (960.0, 640.0);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WindowMode {
    Compact,
    Expanded,
}

/// What the backend actually applied, echoed to the frontend so it can persist
/// the (possibly clamped) expanded size instead of the size it asked for.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowModeReport {
    pub mode: WindowMode,
    pub width: f64,
    pub height: f64,
}

/// Axis-aligned rectangle in physical pixels. Mirrors the monitor work area /
/// window outer frame just enough for the placement math below.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// Requested expanded size normalized against the minimum and the work area
/// (all logical px). An oversized request shrinks to the work area; an
/// undersized one grows to the minimum. When the work area itself is smaller
/// than the minimum, the work area wins — a fully reachable window beats a
/// "big enough" one whose collapse control sits off-screen (the rail's 收起
/// lives at the bottom, and compact would otherwise be unreachable).
pub fn normalize_expanded_size(
    width: Option<f64>,
    height: Option<f64>,
    work_logical: (f64, f64),
) -> (f64, f64) {
    let clamp_axis = |requested: Option<f64>, default: f64, min: f64, work: f64| -> f64 {
        let value = requested.filter(|v| v.is_finite() && *v > 0.0).unwrap_or(default);
        let floor = min.min(work.max(1.0));
        value.clamp(floor, work.max(floor))
    };
    (
        clamp_axis(width, EXPANDED_DEFAULT_SIZE.0, EXPANDED_MIN_SIZE.0, work_logical.0),
        clamp_axis(height, EXPANDED_DEFAULT_SIZE.1, EXPANDED_MIN_SIZE.1, work_logical.1),
    )
}

/// Top-left position (physical px) that keeps `prev`'s center for a window of
/// `target_w`×`target_h`, pulled back inside `work`. Right/bottom clamp first,
/// then left/top, so an oversized window pins to the work area's origin and its
/// title/drag region stays reachable.
pub fn placement(prev: Rect, target_w: f64, target_h: f64, work: Rect) -> (f64, f64) {
    let center_x = prev.x + prev.w / 2.0;
    let center_y = prev.y + prev.h / 2.0;
    let x = (center_x - target_w / 2.0)
        .min(work.x + work.w - target_w)
        .max(work.x);
    let y = (center_y - target_h / 2.0)
        .min(work.y + work.h - target_h)
        .max(work.y);
    (x, y)
}

/// Apply a form factor to the main window. Ordering is load-bearing: the
/// expanded minimum must be lifted *before* shrinking to compact (a 960×640
/// floor would swallow the 400×640 request), and set *after* growing so the
/// grow itself is never constrained by a stale floor.
pub fn apply_window_mode(
    app: &tauri::AppHandle,
    mode: WindowMode,
    width: Option<f64>,
    height: Option<f64>,
) -> Result<WindowModeReport, AppError> {
    let window = app
        .get_webview_window("main")
        .ok_or_else(|| AppError::Internal("main window unavailable".into()))?;
    // A maximized frame swallows set_size on some platforms; leave the
    // maximized state before reshaping (the expanded workbench offers a
    // maximize control, so collapsing from that state is a real path).
    if window.is_maximized().unwrap_or(false) {
        let _ = window.unmaximize();
    }
    let scale = window.scale_factor().unwrap_or(1.0).max(0.1);
    let win_err = |op: &str, e: tauri::Error| AppError::Internal(format!("window {op}: {e}"));

    // Previous outer frame (physical px) — the center we preserve. An
    // undecorated window's outer frame equals its inner size.
    let prev_pos = window.outer_position().map_err(|e| win_err("position", e))?;
    let prev_size = window.outer_size().map_err(|e| win_err("size", e))?;
    let prev = Rect {
        x: prev_pos.x as f64,
        y: prev_pos.y as f64,
        w: prev_size.width as f64,
        h: prev_size.height as f64,
    };

    // Work area of whichever monitor hosts the window right now (fall back to
    // primary, then to a frame around the current position so placement still
    // has something sane to clamp against).
    let monitor = window
        .current_monitor()
        .ok()
        .flatten()
        .or_else(|| window.primary_monitor().ok().flatten());
    let work = monitor
        .as_ref()
        .map(|m| {
            let area = m.work_area();
            Rect {
                x: area.position.x as f64,
                y: area.position.y as f64,
                w: area.size.width as f64,
                h: area.size.height as f64,
            }
        })
        .unwrap_or(Rect {
            x: prev.x,
            y: prev.y,
            w: prev.w.max(COMPACT_SIZE.0 * scale),
            h: prev.h.max(COMPACT_SIZE.1 * scale),
        });

    let (logical_w, logical_h) = match mode {
        WindowMode::Compact => COMPACT_SIZE,
        WindowMode::Expanded => {
            normalize_expanded_size(width, height, (work.w / scale, work.h / scale))
        }
    };

    match mode {
        WindowMode::Compact => {
            window
                .set_min_size(None::<LogicalSize<f64>>)
                .map_err(|e| win_err("min size", e))?;
            window
                .set_size(LogicalSize::new(logical_w, logical_h))
                .map_err(|e| win_err("resize", e))?;
            window
                .set_resizable(false)
                .map_err(|e| win_err("resizable", e))?;
            // Back to the floating popover: shadow is self-drawn around the
            // card, inside the transparent gutter.
            let _ = window.set_shadow(false);
        }
        WindowMode::Expanded => {
            // The workbench fills the frame edge-to-edge (no transparent
            // gutter), so the shadow must come from the OS.
            let _ = window.set_shadow(true);
            window
                .set_resizable(true)
                .map_err(|e| win_err("resizable", e))?;
            window
                .set_size(LogicalSize::new(logical_w, logical_h))
                .map_err(|e| win_err("resize", e))?;
            // On a work area smaller than the nominal minimum the applied size
            // already shrank below it — the floor must follow, or the user
            // could never drag the window back inside the screen.
            window
                .set_min_size(Some(LogicalSize::new(
                    EXPANDED_MIN_SIZE.0.min(logical_w),
                    EXPANDED_MIN_SIZE.1.min(logical_h),
                )))
                .map_err(|e| win_err("min size", e))?;
        }
    }

    let (x, y) = placement(prev, logical_w * scale, logical_h * scale, work);
    window
        .set_position(PhysicalPosition::new(x.round() as i32, y.round() as i32))
        .map_err(|e| win_err("reposition", e))?;

    log::info!(
        "window mode applied mode={mode:?} size={logical_w:.0}x{logical_h:.0} pos={:.0},{:.0}",
        x / scale,
        y / scale
    );
    Ok(WindowModeReport {
        mode,
        width: logical_w,
        height: logical_h,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const WORK: Rect = Rect {
        x: 0.0,
        y: 25.0,
        w: 1728.0,
        h: 1052.0,
    };

    #[test]
    fn placement_keeps_center_when_it_fits() {
        // 400×640 window centered at (864, 551) growing to 1100×720.
        let prev = Rect {
            x: 664.0,
            y: 231.0,
            w: 400.0,
            h: 640.0,
        };
        let (x, y) = placement(prev, 1100.0, 720.0, WORK);
        assert_eq!((x, y), (314.0, 191.0));
    }

    #[test]
    fn placement_clamps_into_work_area() {
        // Window hugging the bottom-right corner must not expand off-screen.
        let prev = Rect {
            x: 1320.0,
            y: 430.0,
            w: 400.0,
            h: 640.0,
        };
        let (x, y) = placement(prev, 1100.0, 720.0, WORK);
        assert_eq!((x, y), (628.0, 357.0));
    }

    #[test]
    fn placement_pins_origin_when_oversized() {
        // Target wider than the work area: left/top edge wins so the drag
        // region stays reachable.
        let prev = Rect {
            x: 0.0,
            y: 25.0,
            w: 1728.0,
            h: 1052.0,
        };
        let (x, y) = placement(prev, 2000.0, 1200.0, WORK);
        assert_eq!((x, y), (WORK.x, WORK.y));
    }

    #[test]
    fn expanded_size_defaults_and_clamps() {
        // Defaults when unspecified.
        assert_eq!(
            normalize_expanded_size(None, None, (1728.0, 1052.0)),
            EXPANDED_DEFAULT_SIZE
        );
        // Remembered size passes through when it fits.
        assert_eq!(
            normalize_expanded_size(Some(1280.0), Some(800.0), (1728.0, 1052.0)),
            (1280.0, 800.0)
        );
        // Oversized request shrinks to the work area.
        assert_eq!(
            normalize_expanded_size(Some(3000.0), Some(2000.0), (1728.0, 1052.0)),
            (1728.0, 1052.0)
        );
        // Undersized / nonsense requests grow to the minimum.
        assert_eq!(
            normalize_expanded_size(Some(100.0), Some(f64::NAN), (1728.0, 1052.0)),
            (EXPANDED_MIN_SIZE.0, EXPANDED_DEFAULT_SIZE.1)
        );
        // A work area smaller than the minimum wins over the minimum: the
        // window must stay fully reachable (1366×768 laptop at 125% scale).
        assert_eq!(
            normalize_expanded_size(None, None, (1092.8, 582.4)),
            (1092.8, 582.4)
        );
    }
}
