//! SGFX executes serially on its owning thread. The display receives a single
//! notification when one or more presents have updated the retained scanout.
use super::{Config, Endpoint, Host};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle};
use wgpu::{Device, Queue, Texture};

enum Event {
    Request,
    Resize(Texture),
    Stop,
}

/// UI-independent SGFX command worker. The display must use the same device
/// and queue for CPU uploads and drawable presentation. Drop off the guest CPU:
/// shutdown joins the host worker and drains accepted GPU submissions.
pub struct Worker {
    events: mpsc::Sender<Event>,
    presented: Arc<Mutex<Option<Texture>>>,
    thread: Option<JoinHandle<()>>,
}

impl Worker {
    pub fn start(
        device: Device,
        queue: Queue,
        mut scanout: Texture,
        config: Config,
        wake_window: impl Fn() + Send + 'static,
    ) -> Result<(Self, Endpoint), String> {
        let (events, incoming) = mpsc::channel();
        let (ready, endpoint) = mpsc::sync_channel(1);
        let presented = Arc::new(Mutex::new(None));
        let pending = Arc::clone(&presented);
        let notify = events.clone();
        let thread = thread::Builder::new()
            .name("scarlet-sgfx".into())
            .spawn(move || {
                let (mut host, endpoint) = match Host::new(device, queue, config, move || {
                    let _ = notify.send(Event::Request);
                }) {
                    Ok(pair) => pair,
                    Err(error) => {
                        let _ = ready.send(Err(error));
                        return;
                    }
                };
                if ready.send(Ok(endpoint)).is_err() {
                    return;
                }
                while let Ok(event) = incoming.recv() {
                    match event {
                        Event::Request => {
                            if host.dispatch(&scanout) {
                                let first =
                                    pending.lock().unwrap().replace(scanout.clone()).is_none();
                                if first {
                                    wake_window();
                                }
                            }
                        }
                        Event::Resize(texture) => scanout = texture,
                        Event::Stop => break,
                    }
                }
            })
            .map_err(|error| error.to_string())?;
        let endpoint = endpoint.recv().map_err(|error| error.to_string())??;
        Ok((
            Self {
                events,
                presented,
                thread: Some(thread),
            },
            endpoint,
        ))
    }

    pub fn resize(&self, texture: &Texture) {
        let _ = self.events.send(Event::Resize(texture.clone()));
    }

    /// Consume a submitted-present notification only for the current scanout.
    /// This is not a GPU fence; display work must follow on the shared queue.
    pub fn take_present(&self, current: &Texture) -> bool {
        self.presented.lock().unwrap().take().as_ref() == Some(current)
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.events.send(Event::Stop);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
