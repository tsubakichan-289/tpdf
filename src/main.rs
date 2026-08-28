use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use clap::Parser;
use crossterm::event::Event;
use tpdf::app::{Action, App, RenderKey, ZoomMode};
use tpdf::event::{AppEvent, spawn_terminal_reader};
use tpdf::pdf::renderer::{self, RenderEvent, RenderRequest};
use tpdf::terminal::session::TerminalSession;
use tpdf::terminal::size::TerminalSize;
use tpdf::{ui, watcher::PdfWatcher};

const DEBOUNCE: Duration = Duration::from_millis(75);
const RETRY_DELAYS: &[Duration] = &[
    Duration::from_millis(50),
    Duration::from_millis(100),
    Duration::from_millis(200),
];

#[derive(Clone, Copy)]
enum TimerAction {
    Reload,
    Retry,
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
    let render_tx = renderer::spawn(event_tx.clone());
    bootstrap(&path, &mut app, &render_tx, &event_rx)?;

    let _watcher = if cli.no_watch {
        None
    } else {
        Some(PdfWatcher::start(&path, event_tx.clone())?)
    };
    let mut terminal = TerminalSession::enter().context("could not initialize terminal")?;
    spawn_terminal_reader(event_tx);

    let mut in_flight = HashSet::new();
    let mut timer = None;
    let mut retry_index = 0;
    request_neighbors(&path, &app, &render_tx, &mut in_flight);
    redraw(&mut terminal, &mut app, &path)?;

    loop {
        let event = receive_until_deadline(&event_rx, timer.map(|(at, _)| at))?;
        let Some(event) = event else {
            let action = timer.take().expect("expired timer has an action").1;
            if matches!(action, TimerAction::Reload) {
                app.reload();
                retry_index = 0;
                in_flight.clear();
            }
            request_page(&path, &app, app.current_page, &render_tx, &mut in_flight);
            redraw(&mut terminal, &mut app, &path)?;
            continue;
        };

        match event {
            AppEvent::Terminal(Event::Key(key)) => match app.handle_key(key) {
                Action::Quit => break,
                Action::Render => {
                    if app.current_bitmap().is_none() {
                        app.status = Some("rendering…".into());
                    }
                    request_page(&path, &app, app.current_page, &render_tx, &mut in_flight);
                    request_neighbors(&path, &app, &render_tx, &mut in_flight);
                    redraw(&mut terminal, &mut app, &path)?;
                }
                Action::Redraw => redraw(&mut terminal, &mut app, &path)?,
                Action::None => {}
            },
            AppEvent::Terminal(Event::Resize(_, _)) => {
                if let Ok(size) = TerminalSize::detect() {
                    app.resize(size);
                    in_flight.clear();
                    request_page(&path, &app, app.current_page, &render_tx, &mut in_flight);
                    redraw(&mut terminal, &mut app, &path)?;
                }
            }
            AppEvent::Terminal(_) => {}
            AppEvent::FileChanged => {
                timer = Some((Instant::now() + DEBOUNCE, TimerAction::Reload));
            }
            AppEvent::WatchError(message) => {
                app.status = Some(format!("watch: {message}"));
                redraw(&mut terminal, &mut app, &path)?;
            }
            AppEvent::Render(render_event) => match render_event {
                RenderEvent::Ready {
                    key,
                    bitmap,
                    page_count,
                    page_size_points,
                } => {
                    in_flight.remove(&key);
                    if key.generation != app.generation {
                        continue;
                    }
                    if key.page == app.current_page {
                        app.set_document(page_count, page_size_points);
                        let expected = app.render_key(app.current_page);
                        if key != expected {
                            request_page(&path, &app, app.current_page, &render_tx, &mut in_flight);
                            continue;
                        }
                    } else {
                        app.page_count = page_count;
                    }
                    app.insert_bitmap(key, bitmap);
                    if key.page == app.current_page {
                        retry_index = 0;
                        request_neighbors(&path, &app, &render_tx, &mut in_flight);
                        redraw(&mut terminal, &mut app, &path)?;
                    }
                }
                RenderEvent::Failed { key, message } if key.generation == app.generation => {
                    in_flight.remove(&key);
                    if key.page == app.current_page {
                        app.status = Some(format!("reload failed: {message}"));
                        if let Some(delay) = RETRY_DELAYS.get(retry_index) {
                            timer = Some((Instant::now() + *delay, TimerAction::Retry));
                            retry_index += 1;
                        }
                        redraw(&mut terminal, &mut app, &path)?;
                    }
                }
                RenderEvent::Failed { .. } => {}
            },
        }
    }
    Ok(())
}

fn bootstrap(
    path: &Path,
    app: &mut App,
    render_tx: &Sender<RenderRequest>,
    event_rx: &Receiver<AppEvent>,
) -> Result<()> {
    loop {
        let key = app.render_key(app.current_page);
        render_tx
            .send(RenderRequest {
                path: path.to_path_buf(),
                key,
            })
            .context("renderer stopped")?;
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

fn request_page(
    path: &Path,
    app: &App,
    page: usize,
    tx: &Sender<RenderRequest>,
    in_flight: &mut HashSet<RenderKey>,
) {
    let key = app.render_key(page);
    if app.has_bitmap(page) || !in_flight.insert(key) {
        return;
    }
    let _ = tx.send(RenderRequest {
        path: path.to_path_buf(),
        key,
    });
}

fn request_neighbors(
    path: &Path,
    app: &App,
    tx: &Sender<RenderRequest>,
    in_flight: &mut HashSet<RenderKey>,
) {
    if app.current_page > 0 {
        request_page(path, app, app.current_page - 1, tx, in_flight);
    }
    if app.current_page + 1 < app.page_count {
        request_page(path, app, app.current_page + 1, tx, in_flight);
    }
}

fn redraw(terminal: &mut TerminalSession, app: &mut App, path: &Path) -> Result<()> {
    if let Some(bitmap) = app.current_bitmap().cloned() {
        terminal.draw_image(&bitmap, app.terminal, app.offset_x, app.offset_y)?;
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
        || std::env::var_os("KITTY_WINDOW_ID").is_some();
    if !compatible {
        bail!(
            "no Kitty graphics capable terminal detected (tpdf is intended for Ghostty; use --force to override)"
        );
    }
    Ok(())
}
