use std::collections::HashMap;

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::pdf::renderer::Bitmap;
use crate::terminal::size::TerminalSize;

pub const ZOOM_STEPS: &[u16] = &[25, 50, 75, 100, 125, 150, 200];

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ZoomMode {
    Fit,
    Fixed(u16),
}

impl ZoomMode {
    pub fn label(self, fit_percent: u16) -> u16 {
        match self {
            Self::Fit => fit_percent,
            Self::Fixed(percent) => percent,
        }
    }

    pub fn zoom_in(&mut self, fit_percent: u16) {
        let current = self.label(fit_percent);
        *self = Self::Fixed(
            ZOOM_STEPS
                .iter()
                .copied()
                .find(|step| *step > current)
                .unwrap_or(*ZOOM_STEPS.last().expect("zoom steps are non-empty")),
        );
    }

    pub fn zoom_out(&mut self, fit_percent: u16) {
        let current = self.label(fit_percent);
        *self = Self::Fixed(
            ZOOM_STEPS
                .iter()
                .rev()
                .copied()
                .find(|step| *step < current)
                .unwrap_or(ZOOM_STEPS[0]),
        );
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct RenderKey {
    pub generation: u64,
    pub page: usize,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Action {
    None,
    Quit,
    Render,
    Redraw,
}

#[derive(Debug)]
pub struct App {
    pub current_page: usize,
    pub page_count: usize,
    pub page_size_points: (f32, f32),
    pub zoom: ZoomMode,
    pub offset_x: u32,
    pub offset_y: u32,
    pub terminal: TerminalSize,
    pub generation: u64,
    pub dirty: bool,
    pub status: Option<String>,
    cache: HashMap<RenderKey, Bitmap>,
}

impl App {
    pub fn new(terminal: TerminalSize) -> Self {
        Self {
            current_page: 0,
            page_count: 0,
            page_size_points: (612.0, 792.0),
            zoom: ZoomMode::Fit,
            offset_x: 0,
            offset_y: 0,
            terminal,
            generation: 0,
            dirty: true,
            status: None,
            cache: HashMap::new(),
        }
    }

    pub fn set_document(&mut self, page_count: usize, page_size_points: (f32, f32)) {
        self.page_count = page_count;
        self.current_page = clamp_page(self.current_page, page_count);
        self.page_size_points = page_size_points;
        self.clamp_offsets();
        self.dirty = true;
    }

    pub fn next_page(&mut self) -> bool {
        if self.current_page + 1 < self.page_count {
            self.current_page += 1;
            self.offset_x = 0;
            self.offset_y = 0;
            self.dirty = true;
            true
        } else {
            false
        }
    }

    pub fn previous_page(&mut self) -> bool {
        if self.current_page > 0 {
            self.current_page -= 1;
            self.offset_x = 0;
            self.offset_y = 0;
            self.dirty = true;
            true
        } else {
            false
        }
    }

    pub fn resize(&mut self, terminal: TerminalSize) {
        if self.terminal != terminal {
            self.terminal = terminal;
            self.clamp_offsets();
            self.dirty = true;
        }
    }

    pub fn reload(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.cache.clear();
        self.status = Some("reloading…".into());
        self.dirty = true;
    }

    pub fn render_dimensions(&self) -> (u32, u32, u16) {
        render_dimensions(
            self.page_size_points,
            self.terminal.viewport_pixels(),
            self.zoom,
        )
    }

    pub fn render_key(&self, page: usize) -> RenderKey {
        let (width, height, _) = self.render_dimensions();
        RenderKey {
            generation: self.generation,
            page,
            width,
            height,
        }
    }

    pub fn current_bitmap(&self) -> Option<&Bitmap> {
        self.cache.get(&self.render_key(self.current_page))
    }

    pub fn has_bitmap(&self, page: usize) -> bool {
        self.cache.contains_key(&self.render_key(page))
    }

    pub fn insert_bitmap(&mut self, key: RenderKey, bitmap: Bitmap) {
        if key.generation != self.generation {
            return;
        }
        self.cache.insert(key, bitmap);
        let current = self.current_page;
        self.cache.retain(|key, _| key.page.abs_diff(current) <= 1);
        if key.page == current {
            self.status = None;
            self.clamp_offsets();
            self.dirty = true;
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Action {
        if key.kind == KeyEventKind::Release {
            return Action::None;
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match (key.code, ctrl) {
            (KeyCode::Char('q'), false) | (KeyCode::Char('c'), true) => Action::Quit,
            (KeyCode::Char('j'), false)
            | (KeyCode::Down, _)
            | (KeyCode::PageDown, _)
            | (KeyCode::Char('d'), true) => {
                self.next_page();
                Action::Render
            }
            (KeyCode::Char('k'), false)
            | (KeyCode::Up, _)
            | (KeyCode::PageUp, _)
            | (KeyCode::Char('u'), true) => {
                self.previous_page();
                Action::Render
            }
            (KeyCode::Char('g'), false) | (KeyCode::Home, _) => {
                self.current_page = 0;
                self.offset_x = 0;
                self.dirty = true;
                Action::Render
            }
            (KeyCode::Char('G'), false) | (KeyCode::End, _) => {
                self.current_page = self.page_count.saturating_sub(1);
                self.offset_x = 0;
                self.dirty = true;
                Action::Render
            }
            (KeyCode::Char('+') | KeyCode::Char('='), false) => {
                let fit = self.render_dimensions().2;
                self.zoom.zoom_in(fit);
                self.offset_x = 0;
                self.dirty = true;
                Action::Render
            }
            (KeyCode::Char('-'), false) => {
                let fit = self.render_dimensions().2;
                self.zoom.zoom_out(fit);
                self.offset_x = 0;
                self.dirty = true;
                Action::Render
            }
            (KeyCode::Char('0'), false) => {
                self.zoom = ZoomMode::Fit;
                self.offset_x = 0;
                self.offset_y = 0;
                self.dirty = true;
                Action::Render
            }
            (KeyCode::Char('h'), false) | (KeyCode::Char('h'), true) => {
                self.offset_x = self.offset_x.saturating_sub(self.scroll_step());
                self.dirty = true;
                Action::Redraw
            }
            (KeyCode::Char('l'), false) => {
                self.offset_x = (self.offset_x + self.scroll_step()).min(self.max_offset_x());
                self.dirty = true;
                Action::Redraw
            }
            (KeyCode::Char('r'), false) | (KeyCode::Char('l'), true) => {
                self.dirty = true;
                Action::Redraw
            }
            _ => Action::None,
        }
    }

    fn scroll_step(&self) -> u32 {
        (self.terminal.viewport_pixels().0 / 8).max(1)
    }

    fn max_offset_x(&self) -> u32 {
        self.render_dimensions()
            .0
            .saturating_sub(self.terminal.viewport_pixels().0)
    }

    fn clamp_offsets(&mut self) {
        self.offset_x = self.offset_x.min(self.max_offset_x());
        self.offset_y = self.offset_y.min(
            self.render_dimensions()
                .1
                .saturating_sub(self.terminal.viewport_pixels().1),
        );
    }
}

pub fn clamp_page(page: usize, page_count: usize) -> usize {
    page.min(page_count.saturating_sub(1))
}

pub fn render_dimensions(
    page_points: (f32, f32),
    viewport_pixels: (u32, u32),
    zoom: ZoomMode,
) -> (u32, u32, u16) {
    let (page_w, page_h) = page_points;
    let (view_w, view_h) = viewport_pixels;
    let fit_scale = (view_w as f32 / page_w)
        .min(view_h as f32 / page_h)
        .max(0.01);
    let fit_percent = (fit_scale * 75.0).round().clamp(1.0, u16::MAX as f32) as u16;
    let scale = match zoom {
        ZoomMode::Fit => fit_scale,
        ZoomMode::Fixed(percent) => (96.0 / 72.0) * f32::from(percent) / 100.0,
    };
    (
        (page_w * scale).round().max(1.0) as u32,
        (page_h * scale).round().max(1.0) as u32,
        fit_percent,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> App {
        let mut app = App::new(TerminalSize::new(100, 40, 800, 800));
        app.set_document(10, (600.0, 800.0));
        app
    }

    #[test]
    fn page_clamp_handles_shrinking_and_empty_documents() {
        assert_eq!(clamp_page(36, 120), 36);
        assert_eq!(clamp_page(36, 30), 29);
        assert_eq!(clamp_page(3, 0), 0);
    }

    #[test]
    fn navigation_stays_in_bounds() {
        let mut app = app();
        assert!(!app.previous_page());
        assert!(app.next_page());
        assert_eq!(app.current_page, 1);
        app.current_page = 9;
        assert!(!app.next_page());
    }

    #[test]
    fn zoom_steps_and_fit_reset_work() {
        let mut zoom = ZoomMode::Fit;
        zoom.zoom_in(82);
        assert_eq!(zoom, ZoomMode::Fixed(100));
        zoom.zoom_out(82);
        assert_eq!(zoom, ZoomMode::Fixed(75));
    }

    #[test]
    fn reload_preserves_current_page_until_metadata_clamps_it() {
        let mut app = app();
        app.current_page = 7;
        app.reload();
        assert_eq!(app.current_page, 7);
        app.set_document(5, (600.0, 800.0));
        assert_eq!(app.current_page, 4);
    }

    #[test]
    fn reload_advances_generation_and_invalidates_bitmap_cache() {
        let mut app = app();
        let key = app.render_key(0);
        app.insert_bitmap(
            key,
            Bitmap {
                width: key.width,
                height: key.height,
                rgba: Vec::new(),
            },
        );
        assert!(app.current_bitmap().is_some());
        app.reload();
        assert_eq!(app.generation, 1);
        assert!(app.current_bitmap().is_none());
    }

    #[test]
    fn fit_to_window_preserves_aspect_ratio() {
        let (w, h, _) = render_dimensions((600.0, 800.0), (1000, 600), ZoomMode::Fit);
        assert_eq!((w, h), (450, 600));
    }

    #[test]
    fn resizing_changes_fit_dimensions() {
        let mut app = app();
        assert_eq!(app.render_dimensions().0, 585);
        app.resize(TerminalSize::new(200, 60, 1600, 1200));
        assert_eq!(app.render_dimensions(), (885, 1180, 111));
    }
}
