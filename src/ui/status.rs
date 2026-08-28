use std::io::{self, Write};
use std::path::Path;

use crossterm::{QueueableCommand, cursor, style, terminal};

use crate::app::App;

pub fn draw<W: Write>(out: &mut W, app: &App, path: &Path) -> io::Result<()> {
    let filename = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("<pdf>");
    let zoom = app.zoom.label(app.render_dimensions().2);
    let message = app.status.as_deref().unwrap_or("");
    let text = format!(
        " {filename}    {} / {}    {zoom}%    {message}",
        app.current_page.saturating_add(1),
        app.page_count
    );
    out.queue(cursor::MoveTo(0, app.terminal.rows.saturating_sub(1)))?
        .queue(style::SetAttribute(style::Attribute::Reverse))?
        .queue(terminal::Clear(terminal::ClearType::CurrentLine))?;
    write!(
        out,
        "{text:.width$}",
        width = usize::from(app.terminal.columns)
    )?;
    out.queue(style::SetAttribute(style::Attribute::Reset))?;
    out.flush()
}
