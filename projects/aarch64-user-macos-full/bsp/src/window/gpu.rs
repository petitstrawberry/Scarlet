//! SGFX executes serially on its owning thread. AppKit receives a single
//! notification when one or more presents have updated the retained scanout.
use crate::gpu::{Endpoint, Host};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle};
use wgpu::{Device, Queue, Texture};

enum Event {
    Request,
    Resize(Texture),
    Stop,
}

pub(super) struct WindowGpu {
    events: mpsc::Sender<Event>,
    presented: Arc<Mutex<Option<Texture>>>,
    thread: Option<JoinHandle<()>>,
}

impl WindowGpu {
    pub fn start(
        device: Device,
        queue: Queue,
        mut scanout: Texture,
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
                let (mut host, endpoint) = match Host::new(device, queue, move || {
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

    pub fn take_present(&self, current: &Texture) -> bool {
        self.presented.lock().unwrap().take().as_ref() == Some(current)
    }
}

impl Drop for WindowGpu {
    fn drop(&mut self) {
        let _ = self.events.send(Event::Stop);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
