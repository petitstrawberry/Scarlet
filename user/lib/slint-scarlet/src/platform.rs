//! Platform implementation for Slint on Scarlet OS

use crate::window_adapter::ScarletWindowAdapter;
use crate::event_loop::EventLoop;
use slint::platform::{Platform, PlatformError, WindowAdapter};
use std::rc::Rc;
use std::cell::RefCell;
use sws_client::Connection;

/// Scarlet OS platform implementation for Slint
pub struct ScarletPlatform {
    connection: RefCell<Connection>,
    event_loop: RefCell<EventLoop>,
}

impl ScarletPlatform {
    /// Create a new ScarletPlatform instance
    pub fn new() -> Result<Self, PlatformError> {
        let connection = Connection::connect("/tmp/sws.sock")
            .map_err(|e| PlatformError::Other(std::format!("Failed to connect to SWS: {:?}", e).into()))?;
        
        let event_loop = EventLoop::new();
        
        Ok(Self { 
            connection: RefCell::new(connection),
            event_loop: RefCell::new(event_loop),
        })
    }
}

impl Platform for ScarletPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, PlatformError> {
        let mut conn = self.connection.borrow_mut();
        let adapter = ScarletWindowAdapter::new(&mut *conn)?;
        
        // Register the window with the event loop
        self.event_loop.borrow_mut().add_window(adapter.clone());
        
        Ok(adapter)
    }

    fn duration_since_start(&self) -> core::time::Duration {
        // TODO: Implement proper time tracking using Scarlet's time APIs
        core::time::Duration::from_secs(0)
    }

    fn run_event_loop(&self) -> Result<(), PlatformError> {
        let mut event_loop = self.event_loop.borrow_mut();
        let mut connection = self.connection.borrow_mut();
        
        event_loop.run(&mut *connection)
    }
}
