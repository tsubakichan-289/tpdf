use std::io::{self, Stdout, Write};

use crossterm::{
    ExecutableCommand, cursor,
    terminal::{self, EnterAlternateScreen, LeaveAlternateScreen},
};

use crate::terminal::kitty::KittyRenderer;

pub struct TerminalSession {
    stdout: Stdout,
    pub kitty: KittyRenderer,
}

impl TerminalSession {
    pub fn enter() -> io::Result<Self> {
        terminal::enable_raw_mode()?;
        let mut stdout = io::stdout();
        if let Err(error) = stdout
            .execute(EnterAlternateScreen)
            .and_then(|out| out.execute(cursor::Hide))
        {
            let _ = terminal::disable_raw_mode();
            return Err(error);
        }
        Ok(Self {
            stdout,
            kitty: KittyRenderer::default(),
        })
    }

    pub fn stdout(&mut self) -> &mut Stdout {
        &mut self.stdout
    }

    pub fn draw_image(
        &mut self,
        bitmap: &crate::pdf::renderer::Bitmap,
        size: crate::terminal::size::TerminalSize,
        offset_x: u32,
        offset_y: u32,
    ) -> io::Result<()> {
        self.kitty
            .draw(&mut self.stdout, bitmap, size, offset_x, offset_y)
    }

    fn restore(&mut self) {
        let _ = self.kitty.delete_visible(&mut self.stdout);
        let _ = self.stdout.execute(cursor::Show);
        let _ = self.stdout.execute(LeaveAlternateScreen);
        let _ = self.stdout.flush();
        let _ = terminal::disable_raw_mode();
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        self.restore();
    }
}
