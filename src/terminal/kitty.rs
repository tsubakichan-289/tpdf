use std::io::{self, Write};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use crossterm::{QueueableCommand, cursor};

use crate::pdf::renderer::Bitmap;
use crate::terminal::size::TerminalSize;

const CHUNK_SIZE: usize = 4096;

pub struct KittyRenderer {
    next_image_id: u32,
    visible_image_id: Option<u32>,
}

impl Default for KittyRenderer {
    fn default() -> Self {
        Self {
            next_image_id: 1,
            visible_image_id: None,
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
    ) -> io::Result<()> {
        let image_id = self.next_image_id;
        self.next_image_id = self.next_image_id.wrapping_add(1).max(1);

        out.write_all(&encode_transmission(
            image_id,
            bitmap.width,
            bitmap.height,
            &bitmap.rgba,
        ))?;

        let (view_w, view_h) = terminal.viewport_pixels();
        let crop_w = bitmap.width.saturating_sub(offset_x).min(view_w).max(1);
        let crop_h = bitmap.height.saturating_sub(offset_y).min(view_h).max(1);
        let (cell_w, cell_h) = terminal.cell_pixels();
        let columns = crop_w.div_ceil(cell_w).min(u32::from(terminal.columns));
        let rows = crop_h
            .div_ceil(cell_h)
            .min(u32::from(terminal.rows.saturating_sub(1)));
        let column = (u32::from(terminal.columns).saturating_sub(columns) / 2) as u16;
        let row = (u32::from(terminal.rows.saturating_sub(1)).saturating_sub(rows) / 2) as u16;

        out.queue(cursor::MoveTo(column, row))?;
        write!(
            out,
            "\x1b_Ga=p,i={image_id},x={offset_x},y={offset_y},w={crop_w},h={crop_h},c={columns},r={rows},C=1,q=2\x1b\\"
        )?;
        if let Some(old_id) = self.visible_image_id.replace(image_id) {
            out.write_all(&encode_delete(old_id))?;
        }
        out.flush()
    }

    pub fn delete_visible<W: Write>(&mut self, out: &mut W) -> io::Result<()> {
        if let Some(image_id) = self.visible_image_id.take() {
            out.write_all(&encode_delete(image_id))?;
            out.flush()?;
        }
        Ok(())
    }
}

pub fn encode_transmission(image_id: u32, width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
    let encoded = STANDARD.encode(rgba);
    let chunks: Vec<&[u8]> = encoded.as_bytes().chunks(CHUNK_SIZE).collect();
    let mut output = Vec::with_capacity(encoded.len() + chunks.len() * 32);
    for (index, chunk) in chunks.iter().enumerate() {
        let more = u8::from(index + 1 < chunks.len());
        if index == 0 {
            output.extend_from_slice(
                format!("\x1b_Ga=t,f=32,s={width},v={height},i={image_id},q=2,m={more};")
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
    format!("\x1b_Ga=d,d=i,i={image_id},q=2\x1b\\").into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_rgba_transmission_has_dimensions_and_chunks() {
        let rgba = vec![255; 4 * 40 * 40];
        let command = encode_transmission(7, 40, 40, &rgba);
        let text = String::from_utf8(command).unwrap();
        assert!(text.starts_with("\x1b_Ga=t,f=32,s=40,v=40,i=7,q=2,m=1;"));
        assert!(text.contains("\x1b_Gm=0;"));
        assert!(text.ends_with("\x1b\\"));
    }

    #[test]
    fn delete_targets_only_our_image_id() {
        assert_eq!(encode_delete(42), b"\x1b_Ga=d,d=i,i=42,q=2\x1b\\".to_vec());
    }
}
