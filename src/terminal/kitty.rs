use std::io::{self, Write};
use std::time::Instant;

use crossterm::{QueueableCommand, cursor};

use crate::pdf::renderer::{Bitmap, PixelFormat, perf_debug_enabled};
use crate::terminal::size::TerminalSize;

const CHUNK_SIZE: usize = 4096;
const PLACEMENT_ID: u32 = 1;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Placement {
    offset_x: u32,
    offset_y: u32,
    crop_width: u32,
    crop_height: u32,
    columns: u32,
    rows: u32,
    column: u16,
    row: u16,
}

#[derive(Clone, Copy, Debug)]
struct VisibleImage {
    image_id: u32,
    content_hash: u64,
    width: u32,
    height: u32,
    format: PixelFormat,
    placement: Placement,
}

pub struct KittyRenderer {
    next_image_id: u32,
    visible: Option<VisibleImage>,
}

impl Default for KittyRenderer {
    fn default() -> Self {
        Self {
            next_image_id: 1,
            visible: None,
        }
    }
}

impl KittyRenderer {
    pub fn draw<W: Write>(
        &mut self,
        out: &mut W,
        bitmap: &Bitmap,
        terminal: TerminalSize,
        offset_x: u32,
        offset_y: u32,
        force_transmit: bool,
    ) -> io::Result<()> {
        let placement = placement(bitmap, terminal, offset_x, offset_y);
        let same_content = self.visible.is_some_and(|visible| {
            visible.content_hash == bitmap.content_hash
                && visible.width == bitmap.width
                && visible.height == bitmap.height
                && visible.format == bitmap.format
        });

        if same_content && !force_transmit {
            let visible = self.visible.expect("same content has a visible image");
            if visible.placement == placement {
                return Ok(());
            }
            let started = Instant::now();
            out.write_all(&encode_delete_placement(visible.image_id))?;
            put(out, visible.image_id, placement)?;
            out.flush()?;
            self.visible = Some(VisibleImage {
                placement,
                ..visible
            });
            log_send(started, 0, false);
            return Ok(());
        }

        let started = Instant::now();
        let image_id = self.next_image_id;
        self.next_image_id = self.next_image_id.wrapping_add(1).max(1);
        out.write_all(&encode_transmission(image_id, bitmap))?;
        put(out, image_id, placement)?;
        if let Some(old) = self.visible {
            out.write_all(&encode_delete(old.image_id))?;
        }
        out.flush()?;
        self.visible = Some(VisibleImage {
            image_id,
            content_hash: bitmap.content_hash,
            width: bitmap.width,
            height: bitmap.height,
            format: bitmap.format,
            placement,
        });
        log_send(started, bitmap.base64_zlib.len(), true);
        Ok(())
    }

    pub fn delete_visible<W: Write>(&mut self, out: &mut W) -> io::Result<()> {
        if let Some(visible) = self.visible.take() {
            out.write_all(&encode_delete(visible.image_id))?;
            out.flush()?;
        }
        Ok(())
    }
}

fn placement(bitmap: &Bitmap, terminal: TerminalSize, offset_x: u32, offset_y: u32) -> Placement {
    let (view_width, view_height) = terminal.viewport_pixels();
    let crop_width = bitmap.width.saturating_sub(offset_x).min(view_width).max(1);
    let crop_height = bitmap
        .height
        .saturating_sub(offset_y)
        .min(view_height)
        .max(1);
    let (cell_width, cell_height) = terminal.cell_pixels();
    let columns = crop_width
        .div_ceil(cell_width)
        .min(u32::from(terminal.columns));
    let rows = crop_height
        .div_ceil(cell_height)
        .min(u32::from(terminal.rows.saturating_sub(1)));
    Placement {
        offset_x,
        offset_y,
        crop_width,
        crop_height,
        columns,
        rows,
        column: (u32::from(terminal.columns).saturating_sub(columns) / 2) as u16,
        row: (u32::from(terminal.rows.saturating_sub(1)).saturating_sub(rows) / 2) as u16,
    }
}

fn put<W: Write>(out: &mut W, image_id: u32, placement: Placement) -> io::Result<()> {
    out.queue(cursor::MoveTo(placement.column, placement.row))?;
    write!(
        out,
        "\x1b_Ga=p,i={image_id},p={PLACEMENT_ID},x={},y={},w={},h={},c={},r={},C=1,q=2\x1b\\",
        placement.offset_x,
        placement.offset_y,
        placement.crop_width,
        placement.crop_height,
        placement.columns,
        placement.rows,
    )?;
    Ok(())
}

fn log_send(started: Instant, base64_bytes: usize, transmitted: bool) {
    if perf_debug_enabled() {
        eprintln!(
            "[tpdf perf] kitty_send duration_ms={:.2} base64_bytes={} transmitted={transmitted}",
            started.elapsed().as_secs_f64() * 1000.0,
            base64_bytes,
        );
    }
}

pub fn encode_transmission(image_id: u32, bitmap: &Bitmap) -> Vec<u8> {
    let chunks: Vec<&[u8]> = bitmap.base64_zlib.chunks(CHUNK_SIZE).collect();
    let mut output = Vec::with_capacity(bitmap.base64_zlib.len() + chunks.len() * 32);
    for (index, chunk) in chunks.iter().enumerate() {
        let more = u8::from(index + 1 < chunks.len());
        if index == 0 {
            output.extend_from_slice(
                format!(
                    "\x1b_Ga=t,f={},s={},v={},i={image_id},q=2,o=z,m={more};",
                    bitmap.format.kitty_code(),
                    bitmap.width,
                    bitmap.height,
                )
                .as_bytes(),
            );
        } else {
            output.extend_from_slice(format!("\x1b_Gm={more};").as_bytes());
        }
        output.extend_from_slice(chunk);
        output.extend_from_slice(b"\x1b\\");
    }
    output
}

pub fn encode_delete(image_id: u32) -> Vec<u8> {
    // Capital I deletes placements and frees the terminal-side image data.
    format!("\x1b_Ga=d,d=I,i={image_id},q=2\x1b\\").into_bytes()
}

fn encode_delete_placement(image_id: u32) -> Vec<u8> {
    format!("\x1b_Ga=d,d=p,i={image_id},p={PLACEMENT_ID},q=2\x1b\\").into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pdf::renderer::prepare_bitmap;

    fn bitmap() -> Bitmap {
        prepare_bitmap(40, 40, vec![255; 4 * 40 * 40]).unwrap()
    }

    #[test]
    fn compressed_rgb_transmission_has_protocol_flags_and_chunks() {
        let mut bitmap = bitmap();
        bitmap.base64_zlib = vec![b'A'; CHUNK_SIZE + 10];
        let command = encode_transmission(7, &bitmap);
        let text = String::from_utf8(command).unwrap();
        assert!(text.starts_with("\x1b_Ga=t,f=24,s=40,v=40,i=7,q=2,o=z,m=1;"));
        assert!(text.contains("\x1b_Gm=0;"));
        assert!(text.ends_with("\x1b\\"));
    }

    #[test]
    fn unchanged_image_and_placement_emit_nothing() {
        let bitmap = bitmap();
        let terminal = TerminalSize::new(80, 24, 800, 480);
        let mut renderer = KittyRenderer::default();
        let mut first = Vec::new();
        renderer
            .draw(&mut first, &bitmap, terminal, 0, 0, false)
            .unwrap();
        let mut second = Vec::new();
        renderer
            .draw(&mut second, &bitmap, terminal, 0, 0, false)
            .unwrap();
        assert!(!first.is_empty());
        assert!(second.is_empty());
    }

    #[test]
    fn changed_offset_reuses_transmitted_image() {
        let bitmap = bitmap();
        let terminal = TerminalSize::new(20, 24, 160, 480);
        let mut renderer = KittyRenderer::default();
        renderer
            .draw(&mut Vec::new(), &bitmap, terminal, 0, 0, false)
            .unwrap();
        let mut output = Vec::new();
        renderer
            .draw(&mut output, &bitmap, terminal, 10, 0, false)
            .unwrap();
        let text = String::from_utf8(output).unwrap();
        assert!(!text.contains("a=t"));
        assert!(text.contains("a=p"));
    }

    #[test]
    fn delete_targets_only_our_image_id() {
        assert_eq!(encode_delete(42), b"\x1b_Ga=d,d=I,i=42,q=2\x1b\\".to_vec());
    }
}
