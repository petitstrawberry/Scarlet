use alloc::string::String;
use alloc::boxed::Box;

use crate::network::traits::{ReceiveHandler, TransmitHandler, NextAction};
use crate::network::packet::NetworkPacket;
use crate::network::error::NetworkError;
use crate::network::pipeline::FlexibleStage;

/// Test echo handler (receive)
/// Just passes packets through
#[derive(Debug)]
pub struct EchoRxHandler {
    stage_name: String,
}

impl EchoRxHandler {
    pub fn new(stage_name: &str) -> Self {
        Self {
            stage_name: String::from(stage_name),
        }
    }
}

impl ReceiveHandler for EchoRxHandler {
    fn handle(&self, _packet: &mut NetworkPacket) -> Result<NextAction, NetworkError> {
        // For testing: just pass packet through and complete
        Ok(NextAction::Complete)
    }
}

/// Test echo handler (transmit)
/// Just passes packets through
#[derive(Debug)]
pub struct EchoTxHandler {
    stage_name: String,
}

impl EchoTxHandler {
    pub fn new(stage_name: &str) -> Self {
        Self {
            stage_name: String::from(stage_name),
        }
    }
}

impl TransmitHandler for EchoTxHandler {
    fn handle(&self, _packet: &mut NetworkPacket) -> Result<NextAction, NetworkError> {
        // For testing: just pass packet through and complete
        Ok(NextAction::Complete)
    }
}

/// Test stage builder
pub struct TestStageBuilder {
    stage_id: String,
    enable_rx: bool,
    enable_tx: bool,
}

impl TestStageBuilder {
    pub fn new(stage_id: &str) -> Self {
        Self {
            stage_id: String::from(stage_id),
            enable_rx: false,
            enable_tx: false,
        }
    }

    pub fn enable_rx(mut self) -> Self {
        self.enable_rx = true;
        self
    }

    pub fn enable_tx(mut self) -> Self {
        self.enable_tx = true;
        self
    }

    pub fn build(self) -> FlexibleStage {
        let rx_handler = if self.enable_rx {
            Some(Box::new(EchoRxHandler::new(&self.stage_id)) as Box<dyn ReceiveHandler>)
        } else {
            None
        };

        let tx_handler = if self.enable_tx {
            Some(Box::new(EchoTxHandler::new(&self.stage_id)) as Box<dyn TransmitHandler>)
        } else {
            None
        };

        FlexibleStage {
            stage_id: self.stage_id,
            rx_handler,
            tx_handler,
        }
    }
}
