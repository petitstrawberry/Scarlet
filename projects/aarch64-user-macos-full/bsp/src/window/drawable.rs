//! Metal may block in nextDrawable until WindowServer retires a surface.
//! Keep that wait off both the input loop and the guest SGFX queue.
use crate::latency_trace::{self, Frame};
use std::sync::{Arc, mpsc};
use std::thread::{self, JoinHandle};
use wgpu::{Surface, SurfaceError, SurfaceTexture};

pub(super) struct Drawable {
    pub trace: Option<Frame>,
    pub result: Result<SurfaceTexture, SurfaceError>,
}

pub(super) struct Acquisition {
    requests: Option<mpsc::Sender<Option<Frame>>>,
    thread: Option<JoinHandle<()>>,
}

impl Acquisition {
    pub fn new(
        surface: Arc<Surface<'static>>,
        deliver: impl Fn(Drawable) + Send + 'static,
    ) -> Result<Self, String> {
        let (requests, pending) = mpsc::channel();
        let thread = thread::Builder::new()
            .name("scarlet-metal-drawable".into())
            .spawn(move || {
                while let Ok(trace) = pending.recv() {
                    latency_trace::surface(trace, "surface_acquire_begin");
                    let result = surface.get_current_texture();
                    latency_trace::surface(trace, "surface_acquire_end");
                    deliver(Drawable { trace, result });
                }
            })
            .map_err(|error| error.to_string())?;
        Ok(Self {
            requests: Some(requests),
            thread: Some(thread),
        })
    }

    pub fn request(&self, trace: Option<Frame>) -> Result<(), String> {
        self.requests
            .as_ref()
            .unwrap()
            .send(trace)
            .map_err(|error| error.to_string())
    }
}

impl Drop for Acquisition {
    fn drop(&mut self) {
        drop(self.requests.take());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
