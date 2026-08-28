use crossterm::terminal;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TerminalSize {
    pub columns: u16,
    pub rows: u16,
    pub width_px: u16,
    pub height_px: u16,
    pub zellij: bool,
}

impl TerminalSize {
    pub const fn new(columns: u16, rows: u16, width_px: u16, height_px: u16) -> Self {
        Self {
            columns,
            rows,
            width_px,
            height_px,
            zellij: false,
        }
    }

    pub const fn new_zellij(columns: u16, rows: u16, width_px: u16, height_px: u16) -> Self {
        Self {
            columns,
            rows,
            width_px,
            height_px,
            zellij: true,
        }
    }

    pub fn detect() -> std::io::Result<Self> {
        let window = terminal::window_size()?;
        let zellij = std::env::var_os("ZELLIJ").is_some()
            || std::env::var_os("ZELLIJ_SESSION_NAME").is_some();
        Ok(Self::from_measurements(
            window.columns,
            window.rows,
            window.width,
            window.height,
            zellij,
        ))
    }

    fn from_measurements(
        columns: u16,
        rows: u16,
        reported_width: u16,
        reported_height: u16,
        zellij: bool,
    ) -> Self {
        if zellij {
            // Zellij can combine pane-local rows/columns with the outer
            // Ghostty window's physical HiDPI size. Do not use those reported
            // pixels: they can overestimate a pane by 1.5-2x and make width-fit
            // renders stall. A fixed 16x32 raster cell is sharp in Ghostty and
            // keeps dimensions proportional to the actual pane cell grid.
            let _ = (reported_width, reported_height);
            Self::new_zellij(
                columns,
                rows,
                columns.saturating_mul(16),
                rows.saturating_mul(32),
            )
        } else {
            let width = if reported_width == 0 {
                columns.saturating_mul(8)
            } else {
                reported_width
            };
            let height = if reported_height == 0 {
                rows.saturating_mul(16)
            } else {
                reported_height
            };
            Self::new(columns, rows, width, height)
        }
    }

    pub fn cell_pixels(self) -> (u32, u32) {
        (
            (u32::from(self.width_px) / u32::from(self.columns.max(1))).max(1),
            (u32::from(self.height_px) / u32::from(self.rows.max(1))).max(1),
        )
    }

    pub fn viewport_pixels(self) -> (u32, u32) {
        let (_, cell_h) = self.cell_pixels();
        (
            u32::from(self.width_px),
            u32::from(self.height_px).saturating_sub(cell_h),
        )
    }

    pub fn cap_render_dimensions(self, width: u32, height: u32) -> (u32, u32) {
        if !self.zellij {
            return (width, height);
        }
        let (view_width, view_height) = self.viewport_pixels();
        // Keep a finite ceiling in multiplexers, where transmitting a very
        // large full-page bitmap stalls every pane. Four viewports still lets
        // the keyboard zoom steps reach 400% on typical document panes.
        let maximum_width = view_width.saturating_mul(4).max(1);
        let maximum_height = view_height.saturating_mul(4).max(1);
        let scale = (maximum_width as f64 / width.max(1) as f64)
            .min(maximum_height as f64 / height.max(1) as f64)
            .min(1.0);
        (
            (width as f64 * scale).round().max(1.0) as u32,
            (height as f64 * scale).round().max(1.0) as u32,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zellij_caps_large_rasters_and_preserves_aspect_ratio() {
        let size = TerminalSize::new_zellij(80, 24, 960, 576);
        assert_eq!(size.cap_render_dimensions(8000, 4000), (3840, 1920));
    }

    #[test]
    fn zellij_missing_pixel_report_uses_hidpi_cell_estimate() {
        let size = TerminalSize::from_measurements(80, 24, 0, 0, true);
        assert_eq!((size.width_px, size.height_px), (1280, 768));
        assert_eq!(size.cell_pixels(), (16, 32));
    }

    #[test]
    fn zellij_outer_window_pixels_do_not_change_pane_raster_size() {
        let size = TerminalSize::from_measurements(40, 20, 3840, 2160, true);
        assert_eq!((size.width_px, size.height_px), (640, 640));
        assert_eq!(size.cell_pixels(), (16, 32));
    }

    #[test]
    fn direct_ghostty_does_not_cap_zoomed_rasters() {
        let size = TerminalSize::new(80, 24, 800, 480);
        assert_eq!(size.cap_render_dimensions(4000, 2000), (4000, 2000));
    }
}
