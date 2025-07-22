use alloc::string::{String, ToString};
use alloc::boxed::Box;
use alloc::collections::BTreeMap;

use crate::network::traits::{ReceiveHandler, TransmitHandler, NextAction, NextStageMatcher};
use crate::network::packet::NetworkPacket;
use crate::network::error::NetworkError;
use crate::network::pipeline::FlexibleStage;

/// Test matcher based on payload first byte
#[derive(Debug)]
pub struct PayloadByteMatcher {
    route_map: BTreeMap<u8, String>,
    default_stage: String,
}

impl PayloadByteMatcher {
    pub fn new(default_stage: &str) -> Self {
        Self {
            route_map: BTreeMap::new(),
            default_stage: String::from(default_stage),
        }
    }

    pub fn add_route(mut self, byte_value: u8, target_stage: &str) -> Self {
        self.route_map.insert(byte_value, String::from(target_stage));
        self
    }
}

impl NextStageMatcher<u8> for PayloadByteMatcher {
    fn get_next_stage(&self, value: u8) -> Result<&str, NetworkError> {
        Ok(self.route_map.get(&value)
            .map(|s| s.as_str())
            .unwrap_or(&self.default_stage))
    }
}

/// Test matcher based on hint values
#[derive(Debug)]
pub struct HintMatcher {
    route_map: BTreeMap<String, String>,
    default_stage: String,
}

impl HintMatcher {
    pub fn new(default_stage: &str) -> Self {
        Self {
            route_map: BTreeMap::new(),
            default_stage: String::from(default_stage),
        }
    }

    pub fn add_route(mut self, hint_value: &str, target_stage: &str) -> Self {
        self.route_map.insert(String::from(hint_value), String::from(target_stage));
        self
    }
}

impl NextStageMatcher<String> for HintMatcher {
    fn get_next_stage(&self, value: String) -> Result<&str, NetworkError> {
        Ok(self.route_map.get(&value)
            .map(|s| s.as_str())
            .unwrap_or(&self.default_stage))
    }
}

/// Test routing handler using NextStageMatcher for payload-based routing
#[derive(Debug)]
pub struct MatcherBasedRxHandler {
    #[allow(dead_code)]
    stage_name: String,
    matcher: Box<dyn NextStageMatcher<u8>>,
}

impl MatcherBasedRxHandler {
    pub fn new(stage_name: &str, matcher: Box<dyn NextStageMatcher<u8>>) -> Self {
        Self {
            stage_name: String::from(stage_name),
            matcher,
        }
    }
}

impl ReceiveHandler for MatcherBasedRxHandler {
    fn handle(&self, packet: &mut NetworkPacket) -> Result<NextAction, NetworkError> {
        if let Some(&first_byte) = packet.payload().first() {
            let next_stage = self.matcher.get_next_stage(first_byte)?;
            Ok(NextAction::JumpTo(String::from(next_stage)))
        } else {
            // Empty payload - use byte 0 as default
            let next_stage = self.matcher.get_next_stage(0)?;
            Ok(NextAction::JumpTo(String::from(next_stage)))
        }
    }
}

/// Test routing handler using NextStageMatcher for hint-based routing
#[derive(Debug)]
pub struct HintMatcherBasedRxHandler {
    #[allow(dead_code)]
    stage_name: String,
    hint_key: String,
    matcher: Box<dyn NextStageMatcher<String>>,
}

impl HintMatcherBasedRxHandler {
    pub fn new(stage_name: &str, hint_key: &str, matcher: Box<dyn NextStageMatcher<String>>) -> Self {
        Self {
            stage_name: String::from(stage_name),
            hint_key: String::from(hint_key),
            matcher,
        }
    }
}

impl ReceiveHandler for HintMatcherBasedRxHandler {
    fn handle(&self, packet: &mut NetworkPacket) -> Result<NextAction, NetworkError> {
        let hint_value = packet.get_hint(&self.hint_key)
            .unwrap_or("default")
            .to_string();
        let next_stage = self.matcher.get_next_stage(hint_value)?;
        Ok(NextAction::JumpTo(String::from(next_stage)))
    }
}

/// Test echo handler (receive) - just passes packets through
#[derive(Debug)]
pub struct EchoRxHandler {
    #[allow(dead_code)]
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

/// Test echo handler (transmit) - just passes packets through
#[derive(Debug)]
pub struct EchoTxHandler {
    #[allow(dead_code)]
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

/// Matcher-based stage builder for routing tests
pub struct MatcherStageBuilder {
    stage_id: String,
    payload_matcher: Option<Box<dyn NextStageMatcher<u8>>>,
    hint_matcher: Option<(String, Box<dyn NextStageMatcher<String>>)>,
}

impl MatcherStageBuilder {
    pub fn new(stage_id: &str) -> Self {
        Self {
            stage_id: String::from(stage_id),
            payload_matcher: None,
            hint_matcher: None,
        }
    }

    pub fn with_payload_matcher(mut self, matcher: Box<dyn NextStageMatcher<u8>>) -> Self {
        self.payload_matcher = Some(matcher);
        self
    }

    pub fn with_hint_matcher(mut self, hint_key: &str, matcher: Box<dyn NextStageMatcher<String>>) -> Self {
        self.hint_matcher = Some((String::from(hint_key), matcher));
        self
    }

    pub fn build(self) -> FlexibleStage {
        let rx_handler = if let Some(matcher) = self.payload_matcher {
            Some(Box::new(MatcherBasedRxHandler::new(&self.stage_id, matcher)) as Box<dyn ReceiveHandler>)
        } else if let Some((hint_key, matcher)) = self.hint_matcher {
            Some(Box::new(HintMatcherBasedRxHandler::new(&self.stage_id, &hint_key, matcher)) as Box<dyn ReceiveHandler>)
        } else {
            Some(Box::new(EchoRxHandler::new(&self.stage_id)) as Box<dyn ReceiveHandler>)
        };

        FlexibleStage {
            stage_id: self.stage_id,
            rx_handler,
            tx_handler: None,
        }
    }
}
