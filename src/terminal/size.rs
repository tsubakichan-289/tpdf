use crossterm::terminal;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TerminalSize {
    pub columns: u16,
    pub rows: u16,
    pub width_px: u16,
    pub height_px: u16,
}

impl TerminalSize {
    pub const fn new(columns: u16, rows: u16, width_px: u16, height_px: u16) -> Self {
        Self {
            columns,
            rows,
            width_px,
            height_px,
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
        Ok(Self::new(window.columns, window.rows, width_px, height_px))
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
}
