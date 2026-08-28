use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use clap::Parser;
use crossterm::event::Event;
use tpdf::app::{Action, App, ZoomMode};
use tpdf::event::{AppEvent, spawn_terminal_reader};
use tpdf::pdf::renderer::{self, RenderEvent, RenderHandle, RenderKind, RenderRequest};
use tpdf::terminal::session::TerminalSession;
use tpdf::terminal::size::TerminalSize;
use tpdf::{ui, watcher::PdfWatcher};

const FILE_DEBOUNCE: Duration = Duration::from_millis(75);
const RESIZE_DEBOUNCE: Duration = Duration::from_millis(50);
const PREFETCH_IDLE: Duration = Duration::from_millis(180);
const RETRY_DELAYS: &[Duration] = &[
    Duration::from_millis(50),
    Duration::from_millis(100),
    Duration::from_millis(200),
];

#[derive(Default)]
struct Deadlines {
    reload: Option<Instant>,
    retry: Option<Instant>,
    resize: Option<Instant>,
    prefetch: Option<Instant>,
}

impl Deadlines {
    fn next(&self) -> Option<Instant> {
        [self.reload, self.retry, self.resize, self.prefetch]
            .into_iter()
            .flatten()
            .min()
    }
}

#[derive(Debug, Parser)]
#[command(version, about)]
struct Cli {
    /// PDF file to display
    file: PathBuf,

    /// Initial page (one-based)
    #[arg(long, default_value_t = 1)]
    page: usize,

    /// Disable automatic reload
    #[arg(long)]
    no_watch: bool,

    /// Initial fixed zoom percentage
    #[arg(long)]
    zoom: Option<u16>,

    /// Run even when the terminal cannot be identified as Kitty-compatible
    #[arg(long, hide = true)]
    force: bool,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    if cli.page == 0 {
        bail!("--page is one-based and must be at least 1");
    }
    ensure_terminal_support(cli.force)?;
    let path = cli
        .file
        .canonicalize()
        .with_context(|| format!("cannot open {}", cli.file.display()))?;
    if !path.is_file() {
        bail!("{} is not a regular file", path.display());
    }

    let size = TerminalSize::detect().context("could not determine terminal dimensions")?;
    let mut app = App::new(size);
    app.current_page = cli.page - 1;
    if let Some(zoom) = cli.zoom {
        if zoom == 0 {
            bail!("--zoom must be greater than zero");
        }
        app.zoom = ZoomMode::Fixed(zoom);
    }

    let (event_tx, event_rx) = mpsc::channel();
    let renderer = renderer::spawn(event_tx.clone());
    bootstrap(&path, &mut app, &renderer, &event_rx)?;

    let _watcher = if cli.no_watch {
        None
    } else {
        Some(PdfWatcher::start(&path, event_tx.clone())?)
    };
    let mut terminal = TerminalSession::enter().context("could not initialize terminal")?;
    spawn_terminal_reader(event_tx);

    let mut deadlines = Deadlines {
        prefetch: Some(Instant::now() + PREFETCH_IDLE),
        ..Deadlines::default()
    };
    let mut retry_index = 0;
    redraw(&mut terminal, &mut app, &path, false)?;

    loop {
        let event = receive_until_deadline(&event_rx, deadlines.next())?;
        let Some(event) = event else {
            process_timers(
                &path,
                &mut app,
                &renderer,
                &mut terminal,
                &mut deadlines,
                &mut retry_index,
            )?;
            continue;
        };

        match event {
            AppEvent::Terminal(Event::Key(key)) => {
                if deadlines.prefetch.is_some() {
                    deadlines.prefetch = Some(Instant::now() + PREFETCH_IDLE);
                }
                match app.handle_key(key) {
                    Action::Quit => break,
                    Action::Render => {
                        if app.current_bitmap().is_none() {
                            app.status = Some("rendering…".into());
                            request_page(
                                &path,
                                &app,
                                app.current_page,
                                RenderKind::Current,
                                &renderer,
                            );
                        }
                        deadlines.prefetch = Some(Instant::now() + PREFETCH_IDLE);
                        redraw(&mut terminal, &mut app, &path, false)?;
                    }
                    Action::Redraw => redraw(&mut terminal, &mut app, &path, false)?,
                    Action::ForceRedraw => redraw(&mut terminal, &mut app, &path, true)?,
                    Action::None => {}
                }
            }
            AppEvent::Terminal(Event::Resize(_, _)) => {
                if let Ok(size) = TerminalSize::detect() {
                    app.resize(size);
                    deadlines.resize = Some(Instant::now() + RESIZE_DEBOUNCE);
                    deadlines.prefetch = None;
                    redraw(&mut terminal, &mut app, &path, false)?;
                }
            }
            AppEvent::Terminal(_) => {}
            AppEvent::FileChanged => {
                deadlines.reload = Some(Instant::now() + FILE_DEBOUNCE);
                deadlines.retry = None;
                deadlines.prefetch = None;
            }
            AppEvent::WatchError(message) => {
                app.status = Some(format!("watch: {message}"));
                redraw(&mut terminal, &mut app, &path, false)?;
            }
            AppEvent::Render(render_event) => match render_event {
                RenderEvent::Ready {
                    key,
                    bitmap,
                    page_count,
                    page_size_points,
                } => {
                    if key.generation != app.generation {
                        continue;
                    }
                    if key.page == app.current_page {
                        app.set_document(page_count, page_size_points);
                        let expected = app.render_key(app.current_page);
                        if key != expected {
                            request_page(
                                &path,
                                &app,
                                app.current_page,
                                RenderKind::Current,
                                &renderer,
                            );
                            continue;
                        }
                    } else {
                        app.page_count = page_count;
                    }
                    app.insert_bitmap(key, bitmap);
                    if key.page == app.current_page {
                        retry_index = 0;
                        deadlines.prefetch = Some(Instant::now() + PREFETCH_IDLE);
                        redraw(&mut terminal, &mut app, &path, false)?;
                    }
                }
                RenderEvent::Failed { key, message } if key.generation == app.generation => {
                    if key.page == app.current_page {
                        app.status = Some(format!("reload failed: {message}"));
                        if let Some(delay) = RETRY_DELAYS.get(retry_index) {
                            deadlines.retry = Some(Instant::now() + *delay);
                            retry_index += 1;
                        }
                        redraw(&mut terminal, &mut app, &path, false)?;
                    }
                }
                RenderEvent::Failed { .. } => {}
            },
        }
    }
    Ok(())
}

fn process_timers(
    path: &Path,
    app: &mut App,
    renderer: &RenderHandle,
    terminal: &mut TerminalSession,
    deadlines: &mut Deadlines,
    retry_index: &mut usize,
) -> Result<()> {
    let now = Instant::now();
    let reload_due = deadlines.reload.is_some_and(|deadline| deadline <= now);
    let resize_due = deadlines.resize.is_some_and(|deadline| deadline <= now);
    let retry_due = deadlines.retry.is_some_and(|deadline| deadline <= now);
    let prefetch_due = deadlines.prefetch.is_some_and(|deadline| deadline <= now);

    if reload_due {
        deadlines.reload = None;
        app.reload();
        *retry_index = 0;
    }
    if resize_due {
        deadlines.resize = None;
    }
    if retry_due {
        deadlines.retry = None;
    }
    if reload_due || resize_due || retry_due {
        request_page(path, app, app.current_page, RenderKind::Current, renderer);
        redraw(terminal, app, path, false)?;
    }
    if prefetch_due {
        deadlines.prefetch = None;
        request_neighbors(path, app, renderer);
    }
    Ok(())
}

fn bootstrap(
    path: &Path,
    app: &mut App,
    renderer: &RenderHandle,
    event_rx: &Receiver<AppEvent>,
) -> Result<()> {
    loop {
        request_page(path, app, app.current_page, RenderKind::Current, renderer);
        match event_rx.recv().context("renderer stopped during startup")? {
            AppEvent::Render(RenderEvent::Ready {
                key,
                bitmap,
                page_count,
                page_size_points,
            }) => {
                app.set_document(page_count, page_size_points);
                let expected = app.render_key(app.current_page);
                if key == expected {
                    app.insert_bitmap(key, bitmap);
                    return Ok(());
                }
            }
            AppEvent::Render(RenderEvent::Failed { message, .. }) => bail!(message),
            _ => {}
        }
    }
}

fn request_page(path: &Path, app: &App, page: usize, kind: RenderKind, renderer: &RenderHandle) {
    if app.has_bitmap(page) {
        return;
    }
    renderer.request(RenderRequest {
        path: path.to_path_buf(),
        key: app.render_key(page),
        kind,
    });
}

fn request_neighbors(path: &Path, app: &App, renderer: &RenderHandle) {
    if app.current_page > 0 {
        request_page(
            path,
            app,
            app.current_page - 1,
            RenderKind::Prefetch,
            renderer,
        );
    }
    if app.current_page + 1 < app.page_count {
        request_page(
            path,
            app,
            app.current_page + 1,
            RenderKind::Prefetch,
            renderer,
        );
    }
}

fn redraw(
    terminal: &mut TerminalSession,
    app: &mut App,
    path: &Path,
    force_transmit: bool,
) -> Result<()> {
    if let Some(bitmap) = app.current_bitmap() {
        terminal.draw_image(
            bitmap,
            app.terminal,
            app.offset_x,
            app.offset_y,
            force_transmit,
        )?;
    }
    ui::status::draw(terminal.stdout(), app, path)?;
    app.dirty = false;
    Ok(())
}

fn receive_until_deadline(
    rx: &Receiver<AppEvent>,
    deadline: Option<Instant>,
) -> Result<Option<AppEvent>> {
    match deadline {
        Some(deadline) => match rx.recv_timeout(deadline.saturating_duration_since(Instant::now()))
        {
            Ok(event) => Ok(Some(event)),
            Err(RecvTimeoutError::Timeout) => Ok(None),
            Err(RecvTimeoutError::Disconnected) => bail!("event channel disconnected"),
        },
        None => Ok(Some(rx.recv().context("event channel disconnected")?)),
    }
}

fn ensure_terminal_support(force: bool) -> Result<()> {
    if force {
        return Ok(());
    }
    let term_program = std::env::var("TERM_PROGRAM")
        .unwrap_or_default()
        .to_ascii_lowercase();
    let term = std::env::var("TERM")
        .unwrap_or_default()
        .to_ascii_lowercase();
    let compatible = term_program.contains("ghostty")
        || term_program.contains("wezterm")
        || term.contains("ghostty")
        || term.contains("kitty")
        || std::env::var_os("KITTY_WINDOW_ID").is_some()
        || std::env::var_os("ZELLIJ").is_some();
    if !compatible {
        bail!(
            "no Kitty graphics capable terminal detected (tpdf is intended for Ghostty; use --force to override)"
        );
    }
    Ok(())
}
