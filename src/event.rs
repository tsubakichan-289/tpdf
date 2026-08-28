use std::sync::mpsc::Sender;
use std::thread;

use crossterm::event::{self, Event};

use crate::pdf::renderer::RenderEvent;

#[derive(Debug)]
pub enum AppEvent {
    Terminal(Event),
    FileChanged,
    WatchError(String),
    Render(RenderEvent),
}

pub fn spawn_terminal_reader(tx: Sender<AppEvent>) {
    thread::Builder::new()
        .name("tpdf-input".into())
        .spawn(move || {
            loop {
                match event::read() {
                    Ok(event) => {
                        if tx.send(AppEvent::Terminal(event)).is_err() {
                            break;
                        }
                    }
                    Err(error) => {
                        let _ = tx.send(AppEvent::WatchError(format!(
                            "terminal input failed: {error}"
                        )));
                        break;
                    }
                }
            }
        })
        .expect("failed to spawn terminal input thread");
}
