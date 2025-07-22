use alloc::string::{String, ToString};
use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use alloc::{format, vec};

use crate::network::traits::{ReceiveHandler, TransmitHandler, NextAction, NextStageMatcher};
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
    routes: BTreeMap<u8, String>,
}

impl TestStageBuilder {
    pub fn new(stage_id: &str) -> Self {
        Self {
            stage_id: String::from(stage_id),
            enable_rx: false,
            enable_tx: false,
            routes: BTreeMap::new(),
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

    /// Add routing rule for specific protocol type
    pub fn add_route(mut self, protocol_type: u8, next_stage: &str) -> Self {
        self.routes.insert(protocol_type, String::from(next_stage));
        self
    }

    pub fn build(self) -> FlexibleStage {
        let rx_handler = if self.enable_rx {
            if self.routes.is_empty() {
                // No routing rules, use simple echo handler
                Some(Box::new(EchoRxHandler::new(&self.stage_id)) as Box<dyn ReceiveHandler>)
            } else {
                // Has routing rules, use routing handler
                let matcher = TestProtocolMatcher::with_custom_routes(self.routes.clone());
                Some(Box::new(TestProtocolRxHandler::with_matcher(&self.stage_id, matcher)) as Box<dyn ReceiveHandler>)
            }
        } else {
            None
        };

        let tx_handler = if self.enable_tx {
            if self.routes.is_empty() {
                // No routing rules, use simple echo handler
                Some(Box::new(EchoTxHandler::new(&self.stage_id)) as Box<dyn TransmitHandler>)
            } else {
                // Has routing rules, use routing handler with default protocol type
                let matcher = TestProtocolMatcher::with_custom_routes(self.routes.clone());
                Some(Box::new(TestProtocolTxHandler::with_matcher(&self.stage_id, matcher, TEST_PROTOCOL_TYPE_A)) as Box<dyn TransmitHandler>)
            }
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

// Test protocol constants
pub const TEST_PROTOCOL_TYPE_A: u8 = 0x01;
pub const TEST_PROTOCOL_TYPE_B: u8 = 0x02;
pub const TEST_PROTOCOL_TYPE_C: u8 = 0x03;

/// Test protocol header (1 byte containing next stage info)
#[derive(Debug, Clone)]
pub struct TestProtocolHeader {
    pub next_type: u8,
}

impl TestProtocolHeader {
    pub fn new(next_type: u8) -> Self {
        Self { next_type }
    }

    pub fn from_bytes(data: &[u8]) -> Result<Self, NetworkError> {
        if data.len() < 1 {
            return Err(NetworkError::insufficient_payload_size(1, data.len()));
        }
        Ok(Self {
            next_type: data[0],
        })
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        vec![self.next_type]
    }
}

/// Test protocol matcher that routes based on header type
#[derive(Debug)]
pub struct TestProtocolMatcher {
    routes: BTreeMap<u8, String>,
}

impl TestProtocolMatcher {
    pub fn new() -> Self {
        let mut routes = BTreeMap::new();
        routes.insert(TEST_PROTOCOL_TYPE_A, "type_a_stage".to_string());
        routes.insert(TEST_PROTOCOL_TYPE_B, "type_b_stage".to_string());
        routes.insert(TEST_PROTOCOL_TYPE_C, "type_c_stage".to_string());
        
        Self { routes }
    }

    pub fn with_custom_routes(routes: BTreeMap<u8, String>) -> Self {
        Self { routes }
    }
}

impl NextStageMatcher<u8> for TestProtocolMatcher {
    fn get_next_stage(&self, value: u8) -> Result<&str, NetworkError> {
        self.routes.get(&value)
            .map(|s| s.as_str())
            .ok_or_else(|| NetworkError::UnsupportedProtocol {
                layer: "test_protocol".to_string(),
                protocol: format!("0x{:02x}", value),
            })
    }
}

/// Test protocol receive handler that parses header and routes to next stage
#[derive(Debug)]
pub struct TestProtocolRxHandler {
    stage_name: String,
    matcher: TestProtocolMatcher,
}

impl TestProtocolRxHandler {
    pub fn new(stage_name: &str) -> Self {
        Self {
            stage_name: String::from(stage_name),
            matcher: TestProtocolMatcher::new(),
        }
    }

    pub fn with_matcher(stage_name: &str, matcher: TestProtocolMatcher) -> Self {
        Self {
            stage_name: String::from(stage_name),
            matcher,
        }
    }

    pub fn with_custom_matcher(stage_name: &str, matcher: TestProtocolMatcher) -> Self {
        Self {
            stage_name: String::from(stage_name),
            matcher,
        }
    }
}

impl ReceiveHandler for TestProtocolRxHandler {
    fn handle(&self, packet: &mut NetworkPacket) -> Result<NextAction, NetworkError> {
        // Validate minimum payload size
        packet.validate_payload_size(1)?;
        
        // Parse header from payload
        let header = TestProtocolHeader::from_bytes(packet.payload())?;
        
        // Extract header and update payload
        let payload_data = packet.payload()[1..].to_vec();
        packet.set_payload(payload_data);
        
        // Add parsed header to packet
        packet.add_header("test_protocol", header.to_bytes());
        
        // Set hint for next stage routing
        packet.set_hint("test_protocol_type", &format!("0x{:02x}", header.next_type));
        
        // Route to next stage based on header type
        let next_stage = self.matcher.get_next_stage(header.next_type)?;
        Ok(NextAction::JumpTo(next_stage.to_string()))
    }
}

/// Test protocol transmit handler that adds header to payload
#[derive(Debug)]
pub struct TestProtocolTxHandler {
    stage_name: String,
    matcher: TestProtocolMatcher,
    default_protocol_type: u8,
}

impl TestProtocolTxHandler {
    pub fn new(stage_name: &str, protocol_type: u8) -> Self {
        Self {
            stage_name: String::from(stage_name),
            matcher: TestProtocolMatcher::new(),
            default_protocol_type: protocol_type,
        }
    }

    pub fn with_matcher(stage_name: &str, matcher: TestProtocolMatcher, default_protocol_type: u8) -> Self {
        Self {
            stage_name: String::from(stage_name),
            matcher,
            default_protocol_type,
        }
    }
}

impl TransmitHandler for TestProtocolTxHandler {
    fn handle(&self, packet: &mut NetworkPacket) -> Result<NextAction, NetworkError> {
        // Create header
        let header = TestProtocolHeader::new(self.default_protocol_type);
        
        // Prepend header to payload
        let mut new_payload = header.to_bytes();
        new_payload.extend_from_slice(packet.payload());
        packet.set_payload(new_payload);
        
        // Add header info to packet
        packet.add_header("test_protocol", header.to_bytes());
        packet.set_hint("test_protocol_type", &format!("0x{:02x}", self.default_protocol_type));
        
        Ok(NextAction::Complete)
    }
}

/// Enhanced test stage builder with protocol support
pub struct TestProtocolStageBuilder {
    stage_id: String,
    handler_type: TestHandlerType,
    custom_routes: Option<BTreeMap<u8, String>>,
}

#[derive(Debug)]
pub enum TestHandlerType {
    Echo,
    ProtocolParser,
    ProtocolGenerator(u8),
}

impl TestProtocolStageBuilder {
    pub fn new(stage_id: &str) -> Self {
        Self {
            stage_id: String::from(stage_id),
            handler_type: TestHandlerType::Echo,
            custom_routes: None,
        }
    }

    pub fn as_protocol_parser(mut self) -> Self {
        self.handler_type = TestHandlerType::ProtocolParser;
        self
    }

    pub fn as_protocol_generator(mut self, protocol_type: u8) -> Self {
        self.handler_type = TestHandlerType::ProtocolGenerator(protocol_type);
        self
    }

    pub fn with_custom_routes(mut self, routes: BTreeMap<u8, String>) -> Self {
        self.custom_routes = Some(routes);
        self
    }

    pub fn build_rx_stage(self) -> FlexibleStage {
        let rx_handler: Box<dyn ReceiveHandler> = match self.handler_type {
            TestHandlerType::Echo => {
                Box::new(EchoRxHandler::new(&self.stage_id))
            }
            TestHandlerType::ProtocolParser => {
                if let Some(routes) = self.custom_routes {
                    let matcher = TestProtocolMatcher::with_custom_routes(routes);
                    Box::new(TestProtocolRxHandler::with_custom_matcher(&self.stage_id, matcher))
                } else {
                    Box::new(TestProtocolRxHandler::new(&self.stage_id))
                }
            }
            TestHandlerType::ProtocolGenerator(_) => {
                Box::new(EchoRxHandler::new(&self.stage_id))
            }
        };

        FlexibleStage {
            stage_id: self.stage_id,
            rx_handler: Some(rx_handler),
            tx_handler: None,
        }
    }

    pub fn build_tx_stage(self) -> FlexibleStage {
        let tx_handler: Box<dyn TransmitHandler> = match self.handler_type {
            TestHandlerType::Echo => {
                Box::new(EchoTxHandler::new(&self.stage_id))
            }
            TestHandlerType::ProtocolParser => {
                Box::new(EchoTxHandler::new(&self.stage_id))
            }
            TestHandlerType::ProtocolGenerator(protocol_type) => {
                Box::new(TestProtocolTxHandler::new(&self.stage_id, protocol_type))
            }
        };

        FlexibleStage {
            stage_id: self.stage_id,
            rx_handler: None,
            tx_handler: Some(tx_handler),
        }
    }

    pub fn build_bidirectional_stage(self) -> FlexibleStage {
        let (rx_handler, tx_handler): (Box<dyn ReceiveHandler>, Box<dyn TransmitHandler>) = match self.handler_type {
            TestHandlerType::Echo => {
                (
                    Box::new(EchoRxHandler::new(&self.stage_id)),
                    Box::new(EchoTxHandler::new(&self.stage_id))
                )
            }
            TestHandlerType::ProtocolParser => {
                let rx = if let Some(routes) = self.custom_routes {
                    let matcher = TestProtocolMatcher::with_custom_routes(routes);
                    Box::new(TestProtocolRxHandler::with_custom_matcher(&self.stage_id, matcher))
                } else {
                    Box::new(TestProtocolRxHandler::new(&self.stage_id))
                };
                (rx, Box::new(EchoTxHandler::new(&self.stage_id)))
            }
            TestHandlerType::ProtocolGenerator(protocol_type) => {
                (
                    Box::new(EchoRxHandler::new(&self.stage_id)),
                    Box::new(TestProtocolTxHandler::new(&self.stage_id, protocol_type))
                )
            }
        };

        FlexibleStage {
            stage_id: self.stage_id,
            rx_handler: Some(rx_handler),
            tx_handler: Some(tx_handler),
        }
    }
}
