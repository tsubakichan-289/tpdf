use std::collections::VecDeque;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::Instant;

use anyhow::{Context, Result, anyhow};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use flate2::{Compression, write::ZlibEncoder};
use pdfium_render::prelude::*;

use crate::app::RenderKey;
use crate::event::AppEvent;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum PixelFormat {
    Rgb,
    Rgba,
}

impl PixelFormat {
    pub const fn kitty_code(self) -> u8 {
        match self {
            Self::Rgb => 24,
            Self::Rgba => 32,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Bitmap {
    pub width: u32,
    pub height: u32,
    pub format: PixelFormat,
    pub base64_zlib: Vec<u8>,
    pub raw_bytes: usize,
    pub compressed_bytes: usize,
    pub content_hash: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RenderKind {
    Current,
    Prefetch,
}

#[derive(Clone, Debug)]
pub struct RenderRequest {
    pub path: PathBuf,
    pub key: RenderKey,
    pub kind: RenderKind,
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

#[derive(Default)]
struct PendingRequests {
    current: Option<RenderRequest>,
    prefetch: VecDeque<RenderRequest>,
}

#[derive(Default)]
struct RenderQueue {
    pending: Mutex<PendingRequests>,
    ready: Condvar,
}

#[derive(Clone)]
pub struct RenderHandle {
    queue: Arc<RenderQueue>,
}

impl RenderHandle {
    pub fn request(&self, request: RenderRequest) {
        let mut pending = self.queue.pending.lock().unwrap_or_else(|e| e.into_inner());
        match request.kind {
            RenderKind::Current => {
                if pending
                    .current
                    .as_ref()
                    .is_some_and(|old| old.key == request.key)
                {
                    return;
                }
                pending.current = Some(request);
                pending.prefetch.clear();
            }
            RenderKind::Prefetch => {
                if pending
                    .current
                    .as_ref()
                    .is_some_and(|item| item.key == request.key)
                    || pending.prefetch.iter().any(|item| item.key == request.key)
                {
                    return;
                }
                pending.prefetch.push_back(request);
                while pending.prefetch.len() > 2 {
                    pending.prefetch.pop_front();
                }
            }
        }
        self.queue.ready.notify_one();
    }
}

pub fn spawn(event_tx: std::sync::mpsc::Sender<AppEvent>) -> RenderHandle {
    let queue = Arc::new(RenderQueue::default());
    let worker_queue = Arc::clone(&queue);
    thread::Builder::new()
        .name("tpdf-renderer".into())
        .spawn(move || {
            let pdfium = bind_pdfium();
            loop {
                let request = next_request(&worker_queue);
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
    RenderHandle { queue }
}

fn next_request(queue: &RenderQueue) -> RenderRequest {
    let mut pending = queue.pending.lock().unwrap_or_else(|e| e.into_inner());
    loop {
        if let Some(request) = pending.current.take() {
            return request;
        }
        if let Some(request) = pending.prefetch.pop_front() {
            return request;
        }
        pending = queue.ready.wait(pending).unwrap_or_else(|e| e.into_inner());
    }
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
    let raster_started = Instant::now();
    let bitmap = page
        .render(width, height, None)
        .context("PDFium page rasterization failed")?;
    let raster_elapsed = raster_started.elapsed();
    let prepared = prepare_bitmap(
        bitmap.width() as u32,
        bitmap.height() as u32,
        bitmap.as_rgba_bytes(),
    )?;
    if perf_debug_enabled() {
        eprintln!(
            "[tpdf perf] raster page={} duration_ms={:.2} format={} raw_bytes={} compressed_bytes={} base64_bytes={}",
            page_index + 1,
            raster_elapsed.as_secs_f64() * 1000.0,
            prepared.format.kitty_code(),
            prepared.raw_bytes,
            prepared.compressed_bytes,
            prepared.base64_zlib.len(),
        );
    }
    Ok(RenderEvent::Ready {
        key: RenderKey {
            page: page_index,
            ..request.key
        },
        bitmap: prepared,
        page_count,
        page_size_points,
    })
}

pub fn prepare_bitmap(width: u32, height: u32, rgba: Vec<u8>) -> Result<Bitmap> {
    let opaque = rgba.chunks_exact(4).all(|pixel| pixel[3] == u8::MAX);
    let (format, pixels) = if opaque {
        let mut rgb = Vec::with_capacity(rgba.len() / 4 * 3);
        for pixel in rgba.chunks_exact(4) {
            rgb.extend_from_slice(&pixel[..3]);
        }
        (PixelFormat::Rgb, rgb)
    } else {
        (PixelFormat::Rgba, rgba)
    };
    let raw_bytes = pixels.len();
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::fast());
    encoder
        .write_all(&pixels)
        .context("zlib compression failed")?;
    let compressed = encoder.finish().context("zlib compression failed")?;
    let compressed_bytes = compressed.len();
    let base64_zlib = STANDARD.encode(compressed).into_bytes();
    let mut hasher = DefaultHasher::new();
    width.hash(&mut hasher);
    height.hash(&mut hasher);
    format.hash(&mut hasher);
    base64_zlib.hash(&mut hasher);
    Ok(Bitmap {
        width,
        height,
        format,
        base64_zlib,
        raw_bytes,
        compressed_bytes,
        content_hash: hasher.finish(),
    })
}

pub fn perf_debug_enabled() -> bool {
    std::env::var_os("TPDF_DEBUG_PERF").is_some_and(|value| value == "1")
}

#[cfg(test)]
mod tests {
    use std::io::Read;

    use flate2::read::ZlibDecoder;

    use super::*;

    fn decode(bitmap: &Bitmap) -> Vec<u8> {
        let compressed = STANDARD.decode(&bitmap.base64_zlib).unwrap();
        let mut decoder = ZlibDecoder::new(compressed.as_slice());
        let mut pixels = Vec::new();
        decoder.read_to_end(&mut pixels).unwrap();
        pixels
    }

    #[test]
    fn opaque_rgba_becomes_compressed_rgb24() {
        let bitmap = prepare_bitmap(2, 1, vec![1, 2, 3, 255, 4, 5, 6, 255]).unwrap();
        assert_eq!(bitmap.format, PixelFormat::Rgb);
        assert_eq!(bitmap.raw_bytes, 6);
        assert_eq!(decode(&bitmap), vec![1, 2, 3, 4, 5, 6]);
    }

    #[test]
    fn transparent_bitmap_remains_rgba() {
        let rgba = vec![1, 2, 3, 128, 4, 5, 6, 255];
        let bitmap = prepare_bitmap(2, 1, rgba.clone()).unwrap();
        assert_eq!(bitmap.format, PixelFormat::Rgba);
        assert_eq!(decode(&bitmap), rgba);
    }

    #[test]
    fn queue_current_replaces_old_work_and_clears_prefetch() {
        let queue = Arc::new(RenderQueue::default());
        let handle = RenderHandle {
            queue: Arc::clone(&queue),
        };
        let request = |page, kind| RenderRequest {
            path: PathBuf::from("test.pdf"),
            key: RenderKey {
                generation: 0,
                page,
                width: 10,
                height: 10,
            },
            kind,
        };
        handle.request(request(0, RenderKind::Prefetch));
        handle.request(request(1, RenderKind::Current));
        handle.request(request(2, RenderKind::Current));
        let pending = queue.pending.lock().unwrap();
        assert_eq!(pending.current.as_ref().unwrap().key.page, 2);
        assert!(pending.prefetch.is_empty());
    }
}
