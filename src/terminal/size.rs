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
        let width_px = if window.width == 0 {
            window.columns.saturating_mul(8)
        } else {
            window.width
        };
        let height_px = if window.height == 0 {
            window.rows.saturating_mul(16)
        } else {
            window.height
        };
        let zellij = std::env::var_os("ZELLIJ").is_some()
            || std::env::var_os("ZELLIJ_SESSION_NAME").is_some();
        if zellij {
            // Zellij can expose outer-terminal pixels with pane-local cells.
            // Bound the effective cell size to avoid a full-window HiDPI render.
            let width_px = width_px.min(window.columns.saturating_mul(12));
            let height_px = height_px.min(window.rows.saturating_mul(24));
            Ok(Self::new_zellij(
                window.columns,
                window.rows,
                width_px,
                height_px,
            ))
        } else {
            Ok(Self::new(window.columns, window.rows, width_px, height_px))
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
        let maximum_width = view_width.saturating_mul(2).max(1);
        let maximum_height = view_height.saturating_mul(2).max(1);
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
        assert_eq!(size.cap_render_dimensions(4000, 2000), (1920, 960));
    }

    #[test]
    fn direct_ghostty_does_not_cap_zoomed_rasters() {
        let size = TerminalSize::new(80, 24, 800, 480);
        assert_eq!(size.cap_render_dimensions(4000, 2000), (4000, 2000));
    }
}
