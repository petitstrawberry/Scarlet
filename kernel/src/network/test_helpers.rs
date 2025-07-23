use alloc::string::{String, ToString};
use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use alloc::{format, vec};

use crate::network::traits::{PacketHandler, NextAction, NextStageMatcher};
use crate::network::packet::NetworkPacket;
use crate::network::error::NetworkError;
use crate::network::pipeline::{FlexibleStage, StageIdentifier};

/// Test protocol stage identifiers for type-safe testing
/// These represent different protocols that use the same underlying implementation
pub struct EchoProtocol;
pub struct EthernetProtocol;
pub struct IpProtocol;
pub struct TcpProtocol;
pub struct UdpProtocol;
pub struct CustomProtocolA;
pub struct CustomProtocolB;

impl StageIdentifier for EchoProtocol {
    fn stage_id() -> &'static str { "echo" }
}

impl StageIdentifier for EthernetProtocol {
    fn stage_id() -> &'static str { "ethernet" }
}

impl StageIdentifier for IpProtocol {
    fn stage_id() -> &'static str { "ip" }
}

impl StageIdentifier for TcpProtocol {
    fn stage_id() -> &'static str { "tcp" }
}

impl StageIdentifier for UdpProtocol {
    fn stage_id() -> &'static str { "udp" }
}

impl StageIdentifier for CustomProtocolA {
    fn stage_id() -> &'static str { "custom_a" }
}

impl StageIdentifier for CustomProtocolB {
    fn stage_id() -> &'static str { "custom_b" }
}

/// Tracing functionality for testing
#[derive(Debug, Clone)]
pub struct PipelineTrace {
    pub stages: Vec<String>,
    pub packet_states: Vec<String>, // Packet state after each stage
}

impl PipelineTrace {
    pub fn new() -> Self {
        Self {
            stages: Vec::new(),
            packet_states: Vec::new(),
        }
    }

    pub fn add_stage(&mut self, stage_name: &str, packet: &NetworkPacket) {
        self.stages.push(String::from(stage_name));
        // Record packet state: payload_len, headers_count, hints_count
        let state = format!("payload:{}, headers:{}, hints:{}", 
            packet.payload().len(),
            packet.headers().len(),
            packet.hints().len()
        );
        self.packet_states.push(state);
    }

    pub fn get_path(&self) -> String {
        self.stages.join(" -> ")
    }
}

/// Test echo handler (receive)
/// Just passes packets through
#[derive(Debug)]
pub struct EchoRxHandler {
    stage_name: String,
    enable_tracing: bool,
}

impl EchoRxHandler {
    pub fn new(stage_name: &str) -> Self {
        Self {
            stage_name: String::from(stage_name),
            enable_tracing: false,
        }
    }

    pub fn with_tracing(stage_name: &str) -> Self {
        Self {
            stage_name: String::from(stage_name),
            enable_tracing: true,
        }
    }
}

impl PacketHandler for EchoRxHandler {
    fn handle(&self, packet: &mut NetworkPacket) -> Result<NextAction, NetworkError> {
        // Add tracing information if enabled
        if self.enable_tracing {
            let trace_key = "pipeline_trace";
            let current_trace = packet.get_hint(trace_key).unwrap_or("");
            let new_trace = if current_trace.is_empty() {
                self.stage_name.clone()
            } else {
                format!("{} -> {}", current_trace, self.stage_name)
            };
            packet.set_hint(trace_key, &new_trace);
            
            // Add stage-specific processing marker
            packet.set_hint(&format!("processed_by_{}", self.stage_name), "true");
        }
        
        // For testing: just pass packet through and complete
        Ok(NextAction::Complete)
    }
}

/// Test echo handler (transmit)
/// Just passes packets through
#[derive(Debug)]
pub struct EchoTxHandler {
    stage_name: String,
    enable_tracing: bool,
}

impl EchoTxHandler {
    pub fn new(stage_name: &str) -> Self {
        Self {
            stage_name: String::from(stage_name),
            enable_tracing: false,
        }
    }

    pub fn with_tracing(stage_name: &str) -> Self {
        Self {
            stage_name: String::from(stage_name),
            enable_tracing: true,
        }
    }
}

impl PacketHandler for EchoTxHandler {
    fn handle(&self, packet: &mut NetworkPacket) -> Result<NextAction, NetworkError> {
        // Add tracing information if enabled
        if self.enable_tracing {
            let trace_key = "pipeline_trace";
            let current_trace = packet.get_hint(trace_key).unwrap_or("");
            let new_trace = if current_trace.is_empty() {
                self.stage_name.clone()
            } else {
                format!("{} -> {}", current_trace, self.stage_name)
            };
            packet.set_hint(trace_key, &new_trace);
            
            // Add stage-specific processing marker
            packet.set_hint(&format!("processed_by_{}", self.stage_name), "true");
        }
        
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
    enable_tracing: bool,
}

impl TestStageBuilder {
    pub fn new(stage_id: &str) -> Self {
        Self {
            stage_id: String::from(stage_id),
            enable_rx: false,
            enable_tx: false,
            routes: BTreeMap::new(),
            enable_tracing: false,
        }
    }
    
    /// Create a new TestStageBuilder with type-safe identifier
    pub fn new_typed<T: StageIdentifier>() -> Self {
        Self {
            stage_id: T::stage_id().to_string(),
            enable_rx: false,
            enable_tx: false,
            routes: BTreeMap::new(),
            enable_tracing: false,
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
    
    /// Add routing rule with type-safe next stage
    pub fn add_route_typed<T: StageIdentifier>(mut self, protocol_type: u8) -> Self {
        self.routes.insert(protocol_type, T::stage_id().to_string());
        self
    }

    /// Enable tracing for this stage
    pub fn enable_tracing(mut self) -> Self {
        self.enable_tracing = true;
        self
    }

    pub fn build(self) -> FlexibleStage {
        let rx_handler = if self.enable_rx {
            if self.routes.is_empty() {
                // No routing rules, use simple echo handler
                if self.enable_tracing {
                    Some(Box::new(EchoRxHandler::with_tracing(&self.stage_id)) as Box<dyn PacketHandler>)
                } else {
                    Some(Box::new(EchoRxHandler::new(&self.stage_id)) as Box<dyn PacketHandler>)
                }
            } else {
                // Has routing rules, use routing handler
                let matcher = TestProtocolMatcher::with_custom_routes(self.routes.clone());
                if self.enable_tracing {
                    Some(Box::new(TestProtocolRxHandler::with_tracing(&self.stage_id, matcher)) as Box<dyn PacketHandler>)
                } else {
                    Some(Box::new(TestProtocolRxHandler::with_matcher(&self.stage_id, matcher)) as Box<dyn PacketHandler>)
                }
            }
        } else {
            None
        };

        let tx_handler = if self.enable_tx {
            if self.routes.is_empty() {
                // No routing rules, use simple echo handler
                Some(Box::new(EchoTxHandler::new(&self.stage_id)) as Box<dyn PacketHandler>)
            } else {
                // Has routing rules, use routing handler with default protocol type
                let matcher = TestProtocolMatcher::with_custom_routes(self.routes.clone());
                Some(Box::new(TestProtocolTxHandler::with_matcher(&self.stage_id, matcher, TEST_PROTOCOL_TYPE_A)) as Box<dyn PacketHandler>)
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
#[derive(Debug, Clone)]
pub struct TestProtocolMatcher {
    routes: BTreeMap<u8, String>,
}

impl TestProtocolMatcher {
    pub fn new() -> Self {
        let routes = BTreeMap::new();
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
    enable_tracing: bool,
}

impl TestProtocolRxHandler {
    pub fn new(stage_name: &str) -> Self {
        Self {
            stage_name: String::from(stage_name),
            matcher: TestProtocolMatcher::new(),
            enable_tracing: false,
        }
    }

    pub fn with_matcher(stage_name: &str, matcher: TestProtocolMatcher) -> Self {
        Self {
            stage_name: String::from(stage_name),
            matcher,
            enable_tracing: false,
        }
    }

    pub fn with_custom_matcher(stage_name: &str, matcher: TestProtocolMatcher) -> Self {
        Self {
            stage_name: String::from(stage_name),
            matcher,
            enable_tracing: false,
        }
    }

    pub fn with_tracing(stage_name: &str, matcher: TestProtocolMatcher) -> Self {
        Self {
            stage_name: String::from(stage_name),
            matcher,
            enable_tracing: true,
        }
    }
}

impl PacketHandler for TestProtocolRxHandler {
    fn handle(&self, packet: &mut NetworkPacket) -> Result<NextAction, NetworkError> {
        // Add tracing information if enabled
        if self.enable_tracing {
            let trace_key = "pipeline_trace";
            let current_trace = packet.get_hint(trace_key).unwrap_or("");
            let new_trace = if current_trace.is_empty() {
                self.stage_name.clone()
            } else {
                format!("{} -> {}", current_trace, self.stage_name)
            };
            packet.set_hint(trace_key, &new_trace);
        }

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
        
        // Add processing marker if tracing enabled
        if self.enable_tracing {
            packet.set_hint(&format!("processed_by_{}", self.stage_name), &format!("protocol_type:0x{:02x}", header.next_type));
        }
        
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
    enable_tracing: bool,
}

impl TestProtocolTxHandler {
    pub fn new(stage_name: &str, protocol_type: u8) -> Self {
        Self {
            stage_name: String::from(stage_name),
            matcher: TestProtocolMatcher::new(),
            default_protocol_type: protocol_type,
            enable_tracing: false,
        }
    }

    pub fn with_matcher(stage_name: &str, matcher: TestProtocolMatcher, default_protocol_type: u8) -> Self {
        Self {
            stage_name: String::from(stage_name),
            matcher,
            default_protocol_type,
            enable_tracing: false,
        }
    }

    pub fn with_tracing(stage_name: &str, matcher: TestProtocolMatcher, default_protocol_type: u8) -> Self {
        Self {
            stage_name: String::from(stage_name),
            matcher,
            default_protocol_type,
            enable_tracing: true,
        }
    }
}

impl PacketHandler for TestProtocolTxHandler {
    fn handle(&self, packet: &mut NetworkPacket) -> Result<NextAction, NetworkError> {
        // Add tracing information if enabled
        if self.enable_tracing {
            let trace_key = "pipeline_trace";
            let current_trace = packet.get_hint(trace_key).unwrap_or("");
            let new_trace = if current_trace.is_empty() {
                self.stage_name.clone()
            } else {
                format!("{} -> {}", current_trace, self.stage_name)
            };
            packet.set_hint(trace_key, &new_trace);
            
            // Add stage-specific processing marker
            packet.set_hint(&format!("processed_by_{}", self.stage_name), &format!("protocol_type:0x{:02x}", self.default_protocol_type));
        }
        
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
    enable_tracing: bool,
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
            enable_tracing: false,
        }
    }
    
    /// Create a new TestProtocolStageBuilder with type-safe identifier
    pub fn new_typed<T: StageIdentifier>() -> Self {
        Self {
            stage_id: T::stage_id().to_string(),
            handler_type: TestHandlerType::Echo,
            custom_routes: None,
            enable_tracing: false,
        }
    }

    pub fn enable_tracing(mut self) -> Self {
        self.enable_tracing = true;
        self
    }

    pub fn as_protocol_parser(mut self) -> Self {
        self.handler_type = TestHandlerType::ProtocolParser;
        self
    }

    pub fn as_protocol_generator(mut self, protocol_type: u8) -> Self {
        self.handler_type = TestHandlerType::ProtocolGenerator(protocol_type);
        self
    }

    pub fn add_route(mut self, protocol_type: u8, next_stage: &str) -> Self {
        if let Some(ref mut routes) = self.custom_routes {
            routes.insert(protocol_type, String::from(next_stage));
        } else {
            let mut routes = BTreeMap::new();
            routes.insert(protocol_type, String::from(next_stage));
            self.custom_routes = Some(routes);
        }
        self
    }
    
    /// Add routing rule with type-safe next stage
    pub fn add_route_typed<T: StageIdentifier>(mut self, protocol_type: u8) -> Self {
        if let Some(ref mut routes) = self.custom_routes {
            routes.insert(protocol_type, T::stage_id().to_string());
        } else {
            let mut routes = BTreeMap::new();
            routes.insert(protocol_type, T::stage_id().to_string());
            self.custom_routes = Some(routes);
        }
        self
    }

    pub fn with_custom_routes(mut self, routes: BTreeMap<u8, String>) -> Self {
        self.custom_routes = Some(routes);
        self
    }

    pub fn build_rx_stage(self) -> FlexibleStage {
        let rx_handler: Box<dyn PacketHandler> = match self.handler_type {
            TestHandlerType::Echo => {
                if self.enable_tracing {
                    Box::new(EchoRxHandler::with_tracing(&self.stage_id))
                } else {
                    Box::new(EchoRxHandler::new(&self.stage_id))
                }
            }
            TestHandlerType::ProtocolParser => {
                if self.enable_tracing {
                    if let Some(routes) = self.custom_routes {
                        let matcher = TestProtocolMatcher::with_custom_routes(routes);
                        Box::new(TestProtocolRxHandler::with_tracing(&self.stage_id, matcher))
                    } else {
                        Box::new(TestProtocolRxHandler::with_tracing(&self.stage_id, TestProtocolMatcher::new()))
                    }
                } else {
                    if let Some(routes) = self.custom_routes {
                        let matcher = TestProtocolMatcher::with_custom_routes(routes);
                        Box::new(TestProtocolRxHandler::with_custom_matcher(&self.stage_id, matcher))
                    } else {
                        Box::new(TestProtocolRxHandler::new(&self.stage_id))
                    }
                }
            }
            TestHandlerType::ProtocolGenerator(_) => {
                if self.enable_tracing {
                    Box::new(EchoRxHandler::with_tracing(&self.stage_id))
                } else {
                    Box::new(EchoRxHandler::new(&self.stage_id))
                }
            }
        };

        FlexibleStage {
            stage_id: self.stage_id,
            rx_handler: Some(rx_handler),
            tx_handler: None,
        }
    }

    pub fn build_tx_stage(self) -> FlexibleStage {
        let tx_handler: Box<dyn PacketHandler> = match self.handler_type {
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
        let (rx_handler, tx_handler): (Box<dyn PacketHandler>, Box<dyn PacketHandler>) = match self.handler_type {
            TestHandlerType::Echo => {
                let rx: Box<dyn PacketHandler> = if self.enable_tracing {
                    Box::new(EchoRxHandler::with_tracing(&self.stage_id))
                } else {
                    Box::new(EchoRxHandler::new(&self.stage_id))
                };
                (rx, Box::new(EchoTxHandler::new(&self.stage_id)))
            }
            TestHandlerType::ProtocolParser => {
                let rx: Box<dyn PacketHandler> = if self.enable_tracing {
                    if let Some(routes) = self.custom_routes {
                        let matcher = TestProtocolMatcher::with_custom_routes(routes);
                        Box::new(TestProtocolRxHandler::with_tracing(&self.stage_id, matcher))
                    } else {
                        Box::new(TestProtocolRxHandler::with_tracing(&self.stage_id, TestProtocolMatcher::new()))
                    }
                } else {
                    if let Some(routes) = self.custom_routes {
                        let matcher = TestProtocolMatcher::with_custom_routes(routes);
                        Box::new(TestProtocolRxHandler::with_custom_matcher(&self.stage_id, matcher))
                    } else {
                        Box::new(TestProtocolRxHandler::new(&self.stage_id))
                    }
                };
                (rx, Box::new(EchoTxHandler::new(&self.stage_id)))
            }
            TestHandlerType::ProtocolGenerator(protocol_type) => {
                let rx: Box<dyn PacketHandler> = if self.enable_tracing {
                    Box::new(EchoRxHandler::with_tracing(&self.stage_id))
                } else {
                    Box::new(EchoRxHandler::new(&self.stage_id))
                };
                (rx, Box::new(TestProtocolTxHandler::new(&self.stage_id, protocol_type)))
            }
        };

        FlexibleStage {
            stage_id: self.stage_id,
            rx_handler: Some(rx_handler),
            tx_handler: Some(tx_handler),
        }
    }
}

/// Echo Stage Builder - Simple echo functionality
pub struct EchoStageBuilder {
    enable_tracing: bool,
}

impl EchoStageBuilder {
    pub fn new() -> Self {
        Self {
            enable_tracing: false,
        }
    }

    pub fn enable_tracing(mut self) -> Self {
        self.enable_tracing = true;
        self
    }

    pub fn build(self) -> FlexibleStage {
        let stage_id = EchoProtocol::stage_id().to_string();
        
        let rx_handler = if self.enable_tracing {
            Some(Box::new(EchoRxHandler::with_tracing(&stage_id)) as Box<dyn PacketHandler>)
        } else {
            Some(Box::new(EchoRxHandler::new(&stage_id)) as Box<dyn PacketHandler>)
        };

        let tx_handler = Some(Box::new(EchoTxHandler::new(&stage_id)) as Box<dyn PacketHandler>);

        FlexibleStage {
            stage_id,
            rx_handler,
            tx_handler,
        }
    }
}

/// Protocol Stage Builder - Protocol parsing and routing functionality
pub struct ProtocolStageBuilder {
    stage_id: String,
    routes: Option<BTreeMap<u8, String>>,
    enable_tracing: bool,
    default_protocol_type: u8,
}

impl ProtocolStageBuilder {
    pub fn new() -> Self {
        Self {
            stage_id: "protocol".to_string(), // Default to "protocol"
            routes: None,
            enable_tracing: false,
            default_protocol_type: TEST_PROTOCOL_TYPE_A,
        }
    }
    
    /// Create with a custom stage identifier
    pub fn with_stage_id(stage_id: &str) -> Self {
        Self {
            stage_id: stage_id.to_string(),
            routes: None,
            enable_tracing: false,
            default_protocol_type: TEST_PROTOCOL_TYPE_A,
        }
    }

    pub fn enable_tracing(mut self) -> Self {
        self.enable_tracing = true;
        self
    }

    pub fn add_route(mut self, protocol_type: u8, next_stage: &str) -> Self {
        if let Some(ref mut routes) = self.routes {
            routes.insert(protocol_type, String::from(next_stage));
        } else {
            let mut routes = BTreeMap::new();
            routes.insert(protocol_type, String::from(next_stage));
            self.routes = Some(routes);
        }
        self
    }

    pub fn add_route_typed<T: StageIdentifier>(mut self, protocol_type: u8) -> Self {
        if let Some(ref mut routes) = self.routes {
            routes.insert(protocol_type, T::stage_id().to_string());
        } else {
            let mut routes = BTreeMap::new();
            routes.insert(protocol_type, T::stage_id().to_string());
            self.routes = Some(routes);
        }
        self
    }

    pub fn with_default_protocol_type(mut self, protocol_type: u8) -> Self {
        self.default_protocol_type = protocol_type;
        self
    }

    pub fn build(self) -> FlexibleStage {
        let stage_id = self.stage_id;
        
        let matcher = if let Some(routes) = self.routes {
            TestProtocolMatcher::with_custom_routes(routes)
        } else {
            TestProtocolMatcher::new()
        };

        let rx_handler = if self.enable_tracing {
            Some(Box::new(TestProtocolRxHandler::with_tracing(&stage_id, matcher.clone())) as Box<dyn PacketHandler>)
        } else {
            Some(Box::new(TestProtocolRxHandler::with_matcher(&stage_id, matcher.clone())) as Box<dyn PacketHandler>)
        };

        let tx_handler = Some(Box::new(TestProtocolTxHandler::with_matcher(&stage_id, matcher, self.default_protocol_type)) as Box<dyn PacketHandler>);

        FlexibleStage {
            stage_id,
            rx_handler,
            tx_handler,
        }
    }
}

/// Drop Stage Builder - Simply completes packet processing (packet termination)
pub struct DropStageBuilder {
    stage_id: String,
    enable_tracing: bool,
}

impl DropStageBuilder {
    pub fn new() -> Self {
        Self {
            stage_id: "drop".to_string(),
            enable_tracing: false,
        }
    }
    
    pub fn with_stage_id(stage_id: &str) -> Self {
        Self {
            stage_id: stage_id.to_string(),
            enable_tracing: false,
        }
    }
    
    pub fn enable_tracing(mut self) -> Self {
        self.enable_tracing = true;
        self
    }
    
    pub fn build(self) -> FlexibleStage {
        let stage_id = self.stage_id;
        
        let rx_handler = Some(Box::new(DropRxHandler::new(&stage_id, self.enable_tracing)) as Box<dyn PacketHandler>);
        let tx_handler = Some(Box::new(DropTxHandler::new(&stage_id, self.enable_tracing)) as Box<dyn PacketHandler>);

        FlexibleStage {
            stage_id,
            rx_handler,
            tx_handler,
        }
    }
}

/// Drop receive handler that simply completes processing
#[derive(Debug)]
pub struct DropRxHandler {
    stage_name: String,
    enable_tracing: bool,
}

impl DropRxHandler {
    pub fn new(stage_name: &str, enable_tracing: bool) -> Self {
        Self {
            stage_name: stage_name.to_string(),
            enable_tracing,
        }
    }
}

impl PacketHandler for DropRxHandler {
    fn handle(&self, packet: &mut NetworkPacket) -> Result<NextAction, NetworkError> {
        // Add tracing information if enabled
        if self.enable_tracing {
            let trace_key = "pipeline_trace";
            let current_trace = packet.get_hint(trace_key).unwrap_or("");
            let new_trace = if current_trace.is_empty() {
                self.stage_name.clone()
            } else {
                format!("{} -> {}", current_trace, self.stage_name)
            };
            packet.set_hint(trace_key, &new_trace);
            
            packet.set_hint(&format!("processed_by_{}", self.stage_name), "dropped");
        }
        
        // Simply complete processing (drop the packet)
        Ok(NextAction::Complete)
    }
}

/// Drop transmit handler that simply completes processing
#[derive(Debug)]
pub struct DropTxHandler {
    stage_name: String,
    enable_tracing: bool,
}

impl DropTxHandler {
    pub fn new(stage_name: &str, enable_tracing: bool) -> Self {
        Self {
            stage_name: stage_name.to_string(),
            enable_tracing,
        }
    }
}

impl PacketHandler for DropTxHandler {
    fn handle(&self, packet: &mut NetworkPacket) -> Result<NextAction, NetworkError> {
        // Add tracing information if enabled
        if self.enable_tracing {
            let trace_key = "pipeline_trace";
            let current_trace = packet.get_hint(trace_key).unwrap_or("");
            let new_trace = if current_trace.is_empty() {
                self.stage_name.clone()
            } else {
                format!("{} -> {}", current_trace, self.stage_name)
            };
            packet.set_hint(trace_key, &new_trace);
            
            packet.set_hint(&format!("processed_by_{}", self.stage_name), "dropped");
        }
        
        // Simply complete processing (drop the packet)
        Ok(NextAction::Complete)
    }
}


