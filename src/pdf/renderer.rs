use std::path::PathBuf;
use std::sync::mpsc::{self, Sender};
use std::thread;

use anyhow::{Context, Result, anyhow};
use pdfium_render::prelude::*;

use crate::app::RenderKey;
use crate::event::AppEvent;

#[derive(Clone, Debug)]
pub struct Bitmap {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct RenderRequest {
    pub path: PathBuf,
    pub key: RenderKey,
}

#[derive(Debug)]
pub enum RenderEvent {
    Ready {
        key: RenderKey,
        bitmap: Bitmap,
        page_count: usize,
        page_size_points: (f32, f32),
    },
    Failed {
        key: RenderKey,
        message: String,
    },
}

pub fn spawn(event_tx: Sender<AppEvent>) -> Sender<RenderRequest> {
    let (tx, rx) = mpsc::channel::<RenderRequest>();
    thread::Builder::new()
        .name("tpdf-renderer".into())
        .spawn(move || {
            let pdfium = bind_pdfium();
            while let Ok(request) = rx.recv() {
                let event = match &pdfium {
                    Ok(pdfium) => render(pdfium, &request),
                    Err(message) => Err(anyhow!(message.clone())),
                };
                let render_event = match event {
                    Ok(event) => event,
                    Err(error) => RenderEvent::Failed {
                        key: request.key,
                        message: format!("{error:#}"),
                    },
                };
                if event_tx.send(AppEvent::Render(render_event)).is_err() {
                    break;
                }
            }
        })
        .expect("failed to spawn renderer thread");
    tx
}

fn bind_pdfium() -> std::result::Result<Pdfium, String> {
    let beside_executable = std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(ToOwned::to_owned))
        .and_then(|directory| {
            Pdfium::bind_to_library(Pdfium::pdfium_platform_library_name_at_path(&directory)).ok()
        });
    beside_executable
        .or_else(|| Pdfium::bind_to_library(Pdfium::pdfium_platform_library_name_at_path(".")).ok())
        .or_else(|| Pdfium::bind_to_system_library().ok())
        .map(Pdfium::new)
        .ok_or_else(|| {
            format!(
                "could not load PDFium. Put {} beside tpdf or install it system-wide",
                Pdfium::pdfium_platform_library_name().to_string_lossy()
            )
        })
}

fn render(pdfium: &Pdfium, request: &RenderRequest) -> Result<RenderEvent> {
    let document = pdfium
        .load_pdf_from_file(&request.path, None)
        .with_context(|| format!("could not open {}", request.path.display()))?;
    let page_count = usize::try_from(document.pages().len()).context("invalid PDF page count")?;
    if page_count == 0 {
        return Err(anyhow!("PDF contains no pages"));
    }
    let page_index = request.key.page.min(page_count - 1);
    let page = document
        .pages()
        .get(i32::try_from(page_index).context("page index exceeds PDFium limit")?)
        .context("could not load PDF page")?;
    let page_size_points = (page.width().value, page.height().value);
    let width = i32::try_from(request.key.width).context("render width is too large")?;
    let height = i32::try_from(request.key.height).context("render height is too large")?;
    let bitmap = page
        .render(width, height, None)
        .context("PDFium page rasterization failed")?;
    Ok(RenderEvent::Ready {
        key: RenderKey {
            page: page_index,
            ..request.key
        },
        bitmap: Bitmap {
            width: bitmap.width() as u32,
            height: bitmap.height() as u32,
            rgba: bitmap.as_rgba_bytes(),
        },
        page_count,
        page_size_points,
    })
}
