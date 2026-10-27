//! Serialized, platform-independent wireless control state machine.
//!
//! Runs in task context behind the control device's sleepable mutex. Backend
//! methods only manipulate bounded queues: they must never block or call back
//! into this core. A worker pumps completions and deadlines independently of UI
//! queries. This module is also compiled directly by the host regression crate.

extern crate alloc;
use super::auth::*;
use alloc::{
    boxed::Box,
    collections::{BTreeMap, VecDeque},
    sync::Arc,
    vec::Vec,
};
use core::sync::atomic::{AtomicBool, Ordering};
use scarlet_abi::wireless::*;
use zeroize::Zeroize;

pub const MAX_BSS: usize = 256;
const SCAN_LIFETIME_NS: u64 = 30_000_000_000;

pub struct BackendRequest {
    pub token: Token,
    pub command: u32,
    pub connect: Connect,
    pub rsn: Option<RsnConnect>,
    pub key: Option<SecretKey>,
    pub key_ref: KeyRef,
}
pub struct BackendEvent {
    pub token: Token,
    pub kind: u32,
    pub result: i32,
    pub bss: Vec<Bss>,
}
pub const BACKEND_COMPLETE: u32 = 1;
pub const BACKEND_ASSOCIATED: u32 = 2;
pub const BACKEND_RESET: u32 = 3;
pub const BACKEND_REMOVED: u32 = 4;

/// Queue-only transport boundary. Key completion means committed hardware state;
/// the mock simulates firmware state and can additionally commit software CCMP keys.
pub trait WirelessBackend: Send {
    /// Attach before publishing the endpoint. Notify after publishing backend
    /// completions, carrier changes or asynchronous security retirement.
    fn attach_notifier(&mut self, _notifier: Arc<dyn WirelessNotifier>) {}
    /// Scheduled simulation/firmware work; real interrupt backends normally
    /// return None. This is an absolute deadline, never a recheck interval.
    fn next_deadline_ns(&self) -> Option<u64> {
        None
    }
    fn capabilities(&self) -> Capabilities;
    fn attach_authentication_port(&mut self, _port: Arc<dyn AuthenticationPort>) {}
    fn submit(&mut self, request: BackendRequest, now_ns: u64) -> Result<(), i32>;
    /// Cancellation is observation, not DMA release. Retain device-owned key
    /// buffers until hardware retirement is confirmed.
    fn cancel(&mut self, token: Token);
    fn poll(&mut self, now_ns: u64) -> Option<BackendEvent>;
    /// Confirmed teardown barrier: all pending/installed keys and old key work
    /// must be retired before returning success. Async backends must arrange
    /// their own barrier; unsupported backends fail closed.
    fn retire_security(&mut self) -> Result<(), i32> {
        Err(NOT_SUPPORTED)
    }
    /// Actual AP and association IEs from this exact firmware connection.
    fn association_rsn(&self, _device: u64, _connection: u64) -> Result<AssociationRsn, i32> {
        Err(NOT_SUPPORTED)
    }
    fn security_stats(&self) -> Result<MockSecurityStats, i32> {
        Err(NOT_SUPPORTED)
    }
    fn inject(&mut self, _step: MockStep) -> Result<(), i32> {
        Err(NOT_SUPPORTED)
    }
}

struct Session {
    ready: Arc<AtomicBool>,
    subscribed: bool,
    sequence: u64,
    overflow: bool,
    events: VecDeque<Event>,
    watermark: u64,
    requests: BTreeMap<u64, Pending>,
}
struct Pending {
    info: RequestInfo,
    token: Token,
    deadline_ns: u64,
}
struct Snapshot {
    expires_ns: u64,
    entries: Vec<Bss>,
}

pub struct Reply {
    pub header: Header,
    pub body: Vec<u8>,
    pub consume: Option<u64>,
    pub resync: bool,
    pub eapol_consume: Option<(Association, u64)>,
}

enum KeyChange {
    Install(KeyMetadata),
    Delete(KeyRef),
    ReplaceGroup(KeyMetadata),
}
struct PendingKey {
    token: Token,
    change: KeyChange,
}
pub struct Radio {
    pub link_id: u64,
    pub device_epoch: u64,
    pub connection_id: u64,
    pub state_seq: u64,
    pub status: Status,
    pub name: [u8; 32],
    backend: Box<dyn WirelessBackend>,
    sessions: BTreeMap<u64, Session>,
    next_session: u64,
    next_connection: u64,
    next_scan: u64,
    owner: Option<u64>,
    connect: Option<Token>,
    scan: Option<Token>,
    disconnect: Option<Token>,
    scans: BTreeMap<u64, Snapshot>,
    port: Option<Arc<dyn AuthenticationPort>>,
    pub peer: [u8; 6],
    external: bool,
    rsn: Option<RsnConnect>,
    keys: KeyState,
    key_work: BTreeMap<(u64, u64), PendingKey>,
    security_clean: bool,
    eapol_seen: u64,
}

fn is_connect(command: u32) -> bool {
    matches!(command, CONNECT | CONNECT_RSN)
}

impl Radio {
    pub fn attach_notifier(&mut self, notifier: Arc<dyn WirelessNotifier>) {
        self.backend.attach_notifier(notifier);
    }
    pub fn next_deadline_ns(&self) -> Option<u64> {
        self.sessions
            .values()
            .flat_map(|s| s.requests.values())
            .filter(|p| p.info.result == ACCEPTED)
            .map(|p| p.deadline_ns)
            .chain(self.scans.values().map(|s| s.expires_ns))
            .chain(self.backend.next_deadline_ns())
            .min()
    }
    pub fn has_ready_sessions(&self) -> bool {
        self.sessions
            .values()
            .any(|s| s.ready.load(Ordering::Acquire))
    }
    pub fn new(link_id: u64, name: [u8; 32], backend: Box<dyn WirelessBackend>) -> Self {
        Self {
            link_id,
            name,
            backend,
            device_epoch: 1,
            connection_id: 0,
            state_seq: 1,
            status: Status {
                availability: RADIO_READY,
                ..Status::default()
            },
            sessions: BTreeMap::new(),
            next_session: 1,
            next_connection: 1,
            next_scan: 1,
            owner: None,
            connect: None,
            scan: None,
            disconnect: None,
            scans: BTreeMap::new(),
            port: None,
            peer: [0; 6],
            external: false,
            rsn: None,
            keys: KeyState::default(),
            key_work: BTreeMap::new(),
            security_clean: true,
            eapol_seen: 0,
        }
    }
    pub fn attach_authentication_port(&mut self, port: Arc<dyn AuthenticationPort>) {
        self.backend.attach_authentication_port(port.clone());
        self.port = Some(port);
    }
    pub fn association(&self) -> Option<Association> {
        (self.external
            && self.connection_id != 0
            && valid_peer(self.peer)
            && matches!(self.status.phase, ASSOCIATED | AUTHORIZED))
        .then_some(Association {
            device: self.device_epoch,
            connection: self.connection_id,
            peer: self.peer,
        })
    }
    fn block_data(&mut self, now: u64) -> Result<(), i32> {
        if let Some(port) = &self.port {
            port.block()?;
        }
        if self.status.phase == AUTHORIZED {
            self.status.link_epoch = self.status.link_epoch.saturating_add(1);
            self.change_phase(
                ASSOCIATED,
                Token {
                    connection: self.connection_id,
                    ..Token::default()
                },
                now,
            );
        }
        Ok(())
    }
    fn retire_security(&mut self, now: u64) {
        // This ordering is mandatory: no data admission while deleting keys.
        let blocked = self.block_data(now);
        for work in self.key_work.values() {
            self.backend.cancel(work.token);
        }
        self.key_work.clear();
        self.keys.clear();
        if self.backend.capabilities().features & FEATURE_KEYS != 0 {
            self.security_clean = blocked.is_ok() && self.backend.retire_security().is_ok();
        }
    }
    pub fn security_status(&self) -> SecurityStatus {
        let (sequence, pending, _) = self
            .port
            .as_ref()
            .map_or((0, 0, false), |p| p.eapol_state());
        SecurityStatus {
            key_revision: self.keys.revision,
            rx_sequence: sequence,
            keys_installed: self.keys.installed.len() as u32,
            keys_pending: self.key_work.len() as u32,
            eapol_pending: pending as u32,
            peer: self.peer,
            flags: self.keys.flags()
                | if self.status.phase != AUTHORIZED {
                    SECURITY_BLOCKED
                } else {
                    0
                }
                | if !self.security_clean {
                    SECURITY_RETIRE_FAILED
                } else {
                    0
                },
            reserved: [0; 2],
        }
    }
    pub fn open(&mut self) -> Result<u64, i32> {
        if self.status.availability == RADIO_REMOVED {
            return Err(DEVICE_GONE);
        }
        if self.sessions.len() >= MAX_SESSIONS {
            return Err(RESOURCE_LIMIT);
        }
        let id = self.next_session;
        self.next_session = id.checked_add(1).ok_or(RESOURCE_LIMIT)?;
        self.sessions.insert(
            id,
            Session {
                ready: Arc::new(AtomicBool::new(false)),
                subscribed: false,
                sequence: 0,
                overflow: false,
                events: VecDeque::new(),
                watermark: 0,
                requests: BTreeMap::new(),
            },
        );
        Ok(id)
    }
    pub fn close(&mut self, session: u64, now: u64) {
        if self.owner == Some(session) {
            self.release(now);
        }
        self.sessions.remove(&session);
    }
    pub fn ready(&self, session: u64) -> bool {
        self.sessions
            .get(&session)
            .is_some_and(|s| s.overflow || !s.events.is_empty())
            || self.status.availability == RADIO_REMOVED
    }
    pub fn readiness(&self, session: u64) -> Arc<AtomicBool> {
        Arc::clone(&self.sessions.get(&session).unwrap().ready)
    }
    fn event(&mut self, kind: u32, result: i32, token: Token, object_id: u64, now: u64) {
        let event = Event {
            timestamp_ns: now,
            link_id: self.link_id,
            device_epoch: self.device_epoch,
            connection_id: token.connection,
            request_id: token.request,
            state_seq: self.state_seq,
            network_generation: self.status.network_generation,
            kind,
            result,
            phase: self.status.phase,
            object_id,
            ..Event::default()
        };
        for session in self.sessions.values_mut().filter(|s| s.subscribed) {
            session.ready.store(true, Ordering::Release);
            let Some(sequence) = session.sequence.checked_add(1) else {
                session.overflow = true;
                continue;
            };
            session.sequence = sequence;
            if session.events.len() == MAX_EVENTS {
                session.overflow = true;
                continue;
            }
            session.events.push_back(Event { sequence, ..event });
        }
    }
    fn change_phase(&mut self, phase: u32, token: Token, now: u64) {
        let Some(next) = self.state_seq.checked_add(1) else {
            self.status.availability = RADIO_REMOVED;
            self.status.phase = IDLE;
            self.connection_id = 0;
            for session in self.sessions.values() {
                session.ready.store(true, Ordering::Release);
            }
            self.event(EVENT_REMOVED, RESOURCE_LIMIT, token, 0, now);
            return;
        };
        self.status.phase = phase;
        self.state_seq = next;
        self.event(EVENT_STATE, OK, token, 0, now);
    }
    fn finish(&mut self, token: Token, result: i32, object_id: u64, now: u64) {
        let Some(pending) = self
            .sessions
            .get_mut(&token.session)
            .and_then(|s| s.requests.get_mut(&token.request))
        else {
            return;
        };
        if pending.token != token || pending.info.result != ACCEPTED {
            return;
        }
        pending.info.result = result;
        pending.info.object_id = object_id;
        self.status.last_result = result;
        self.event(EVENT_COMPLETE, result, token, object_id, now);
    }
    fn invalidate(&mut self, result: i32, now: u64) {
        let key_tokens: Vec<Token> = self
            .sessions
            .values()
            .flat_map(|s| s.requests.values())
            .filter(|p| {
                p.info.result == ACCEPTED && matches!(p.info.command, INSTALL_KEY | DELETE_KEY)
            })
            .map(|p| p.token)
            .collect();
        for token in &key_tokens {
            self.backend.cancel(*token);
        }
        self.retire_security(now);
        for token in key_tokens {
            self.finish(token, result, 0, now);
        }
        for token in [
            self.connect.take(),
            self.scan.take(),
            self.disconnect.take(),
        ]
        .into_iter()
        .flatten()
        {
            self.backend.cancel(token);
            self.finish(token, result, 0, now);
        }
        self.status.connect_request = 0;
        self.status.scan_request = 0;
        self.connection_id = 0;
        self.peer = [0; 6];
        self.external = false;
        self.rsn = None;
        self.status.link_epoch = self.status.link_epoch.saturating_add(1);
        self.change_phase(IDLE, Token::default(), now);
    }
    fn release(&mut self, now: u64) {
        self.invalidate(CANCELED, now);
        self.scans.clear();
        self.owner = None;
    }
    pub fn pump(&mut self, now: u64) {
        if self.external && self.connection_id != 0 {
            let (sequence, _, fault) = self
                .port
                .as_ref()
                .map_or((0, 0, false), |p| p.eapol_state());
            let overflow = self
                .owner
                .and_then(|s| self.sessions.get(&s))
                .is_some_and(|s| s.overflow);
            if fault || overflow {
                self.invalidate(FAILED, now);
            } else if sequence != self.eapol_seen {
                self.eapol_seen = sequence;
                self.event(
                    EVENT_EAPOL,
                    OK,
                    Token {
                        connection: self.connection_id,
                        ..Token::default()
                    },
                    sequence,
                    now,
                );
                if self
                    .owner
                    .and_then(|s| self.sessions.get(&s))
                    .is_some_and(|s| s.overflow)
                {
                    self.invalidate(FAILED, now);
                }
            }
        }
        // Deadlines win when callbacks arrive at or after their deadline.
        let expired: Vec<Token> = self
            .sessions
            .values()
            .flat_map(|s| s.requests.values())
            .filter(|p| p.info.result == ACCEPTED && p.deadline_ns <= now)
            .map(|p| p.token)
            .collect();
        for token in expired {
            if self.key_work.contains_key(&(token.session, token.request)) {
                self.invalidate(TIMED_OUT, now);
                continue;
            }
            if self.scan == Some(token) {
                self.scan = None;
                self.status.scan_request = 0;
            }
            if self.connect == Some(token) || self.disconnect == Some(token) {
                self.invalidate(TIMED_OUT, now);
                continue;
            }
            self.backend.cancel(token);
            self.finish(token, TIMED_OUT, 0, now);
        }
        self.scans.retain(|_, s| s.expires_ns > now);
        for _ in 0..MAX_REQUESTS {
            let Some(event) = self.backend.poll(now) else {
                break;
            };
            self.apply(event, now);
        }
    }
    pub fn apply(&mut self, event: BackendEvent, now: u64) {
        if self.status.availability == RADIO_REMOVED {
            return;
        }
        if event.kind == BACKEND_RESET || event.kind == BACKEND_REMOVED {
            // Unsolicited lifecycle notifications must still match the live epoch.
            if event.token.epoch != self.device_epoch {
                return;
            }
            self.invalidate(STALE_DEVICE, now);
            self.scans.clear();
            let Some(epoch) = self.device_epoch.checked_add(1) else {
                self.status.availability = RADIO_REMOVED;
                return;
            };
            self.device_epoch = epoch;
            if event.kind == BACKEND_REMOVED {
                self.status.availability = RADIO_REMOVED;
            }
            if event.kind == BACKEND_REMOVED {
                for s in self.sessions.values() {
                    s.ready.store(true, Ordering::Release);
                }
            }
            self.event(
                if event.kind == BACKEND_RESET {
                    EVENT_RESET
                } else {
                    EVENT_REMOVED
                },
                if event.kind == BACKEND_RESET {
                    OK
                } else {
                    DEVICE_GONE
                },
                Token::default(),
                0,
                now,
            );
            return;
        }
        let token = event.token;
        if token.epoch != self.device_epoch || self.owner != Some(token.session) {
            return;
        }
        let Some(p) = self
            .sessions
            .get(&token.session)
            .and_then(|s| s.requests.get(&token.request))
        else {
            return;
        };
        if p.token != token || p.info.result != ACCEPTED {
            return;
        }
        let command = p.info.command;
        if matches!(command, INSTALL_KEY | DELETE_KEY | REPLACE_GROUP_KEY) {
            if event.kind != BACKEND_COMPLETE {
                return;
            }
            let Some(work) = self.key_work.remove(&(token.session, token.request)) else {
                return;
            };
            if work.token != token || token.connection != self.connection_id {
                return;
            }
            if event.result != OK {
                self.finish(token, event.result, 0, now);
                self.invalidate(event.result, now);
                return;
            }
            match work.change {
                KeyChange::Install(key) => self.keys.installed(key),
                KeyChange::Delete(key) => self.keys.deleted(key),
                KeyChange::ReplaceGroup(key) => self.keys.replaced_group(key),
            }
            self.finish(token, OK, 0, now);
            return;
        }
        if event.kind == BACKEND_ASSOCIATED {
            if self.connect == Some(token) && self.status.phase != AUTHORIZED {
                if self.external {
                    let Some(peer) = event
                        .bss
                        .first()
                        .map(|b| b.bssid)
                        .filter(|p| valid_peer(*p))
                    else {
                        self.invalidate(FAILED, now);
                        return;
                    };
                    if self.peer != [0; 6] && self.peer != peer {
                        self.invalidate(FAILED, now);
                        return;
                    }
                    self.peer = peer;
                }
                self.change_phase(ASSOCIATED, token, now);
            }
            return;
        }
        if event.kind != BACKEND_COMPLETE {
            return;
        }
        if command == SCAN && self.scan == Some(token) {
            self.scan = None;
            self.status.scan_request = 0;
            if event.result != OK {
                self.finish(token, event.result, 0, now);
                return;
            }
            if event.bss.len() > MAX_BSS
                || event
                    .bss
                    .iter()
                    .any(|b| b.ssid_len > 32 || b.reserved != 0 || b.reserved2 != 0)
            {
                self.finish(token, FAILED, 0, now);
                return;
            }
            let Some(next) = self.next_scan.checked_add(1) else {
                self.finish(token, RESOURCE_LIMIT, 0, now);
                return;
            };
            if self.scans.len() >= MAX_SCANS {
                self.finish(token, RESOURCE_LIMIT, 0, now);
                return;
            }
            let id = self.next_scan;
            self.next_scan = next;
            self.scans.insert(
                id,
                Snapshot {
                    expires_ns: now.saturating_add(SCAN_LIFETIME_NS),
                    entries: event.bss,
                },
            );
            self.event(EVENT_SCAN, OK, token, id, now);
            self.finish(token, OK, id, now);
        } else if is_connect(command)
            && self.connect == Some(token)
            && token.connection == self.connection_id
        {
            if event.result != OK {
                self.invalidate(event.result, now);
                return;
            }
            if event.result == OK && self.external {
                return;
            } // Explicit authorization is mandatory.
            if event.result == OK && self.backend.capabilities().features & FEATURE_DATA != 0 {
                self.change_phase(AUTHORIZED, token, now);
                return; // Finish only after the data facade publishes readiness.
            }
            self.connect = None;
            self.status.connect_request = 0;
            if event.result == OK {
                self.change_phase(AUTHORIZED, token, now);
            } else {
                self.connection_id = 0;
                self.status.link_epoch = self.status.link_epoch.saturating_add(1);
                self.change_phase(IDLE, token, now);
            }
            self.finish(token, event.result, 0, now);
        } else if command == DISCONNECT && self.disconnect == Some(token) {
            self.disconnect = None;
            self.connection_id = 0;
            self.change_phase(IDLE, token, now);
            self.finish(token, event.result, 0, now);
        }
    }
    pub fn complete_data_authorization(&mut self, success: bool, now: u64) {
        if !success {
            self.invalidate(FAILED, now);
            self.status.last_result = FAILED;
            return;
        }
        if self.status.phase == AUTHORIZED {
            if let Some(token) = self.connect.take() {
                self.status.connect_request = 0;
                self.finish(token, OK, 0, now);
            }
        }
    }

    fn reply(&self, mut header: Header, result: i32, body: Vec<u8>) -> Reply {
        header.abi_version = ABI_VERSION;
        header.header_len = HEADER_SIZE as u16;
        header.buffer_len = (HEADER_SIZE + body.len()) as u32;
        header.link_id = self.link_id;
        header.device_epoch = self.device_epoch;
        header.connection_id = self.connection_id;
        header.expected_state_seq = self.state_seq;
        header.flags = 0;
        header.reserved = 0;
        header.result = result;
        Reply {
            header,
            body,
            consume: None,
            resync: false,
            eapol_consume: None,
        }
    }
    fn output(&self, header: Header, body: Vec<u8>) -> Reply {
        if (header.buffer_len as usize) < HEADER_SIZE + body.len() {
            let mut reply = self.reply(header, BUFFER_TOO_SMALL, Vec::new());
            reply.header.buffer_len = (HEADER_SIZE + body.len()) as u32;
            reply
        } else {
            self.reply(header, OK, body)
        }
    }
    fn reserve(
        &mut self,
        session: u64,
        header: Header,
        command: u32,
        timeout_ms: u32,
        now: u64,
    ) -> Result<Token, i32> {
        if timeout_ms > MAX_TIMEOUT_MS {
            return Err(INVALID);
        }
        let s = self.sessions.get_mut(&session).ok_or(DEVICE_GONE)?;
        if header.request_id == 0 || header.request_id <= s.watermark {
            return Err(INVALID);
        }
        if s.requests.len() >= MAX_REQUESTS {
            return Err(RESOURCE_LIMIT);
        }
        let token = Token {
            session,
            request: header.request_id,
            epoch: self.device_epoch,
            connection: self.connection_id,
        };
        let duration = if timeout_ms == 0 {
            DEFAULT_TIMEOUT_MS
        } else {
            timeout_ms
        };
        s.watermark = header.request_id;
        s.requests.insert(
            header.request_id,
            Pending {
                token,
                info: RequestInfo {
                    id: header.request_id,
                    connection_id: token.connection,
                    command,
                    result: ACCEPTED,
                    object_id: 0,
                },
                deadline_ns: now.saturating_add(u64::from(duration) * 1_000_000),
            },
        );
        Ok(token)
    }
    /// Dispatch a copied command. Only commit a returned event cursor after the
    /// response was copied successfully; the device serializes this transaction.
    pub fn command(
        &mut self,
        session: u64,
        command: u32,
        header: Header,
        payload: &[u8],
        now: u64,
    ) -> Reply {
        let error = |this: &Self, code| this.reply(header, code, Vec::new());
        if !header.valid() || payload.len() + HEADER_SIZE != header.buffer_len as usize {
            return error(self, INVALID);
        }
        if !self.sessions.contains_key(&session) {
            return error(self, DEVICE_GONE);
        }
        let query = matches!(
            command,
            GET_INFO
                | GET_CAPABILITIES
                | GET_STATUS
                | GET_REQUEST
                | GET_SCAN_RESULTS
                | DRAIN_EVENTS
                | RESYNC
                | GET_SECURITY_STATUS
                | GET_ASSOCIATION_RSN
                | GET_CCMP_STATUS
        );
        if header.link_id != self.link_id && !(query && header.link_id == 0) {
            return error(self, STALE_DEVICE);
        }
        if header.device_epoch != self.device_epoch && !(query && header.device_epoch == 0) {
            return error(self, STALE_DEVICE);
        }
        if !query && self.status.availability == RADIO_REMOVED {
            return error(self, DEVICE_GONE);
        }
        if !matches!(
            command,
            SCAN | CONNECT
                | CONNECT_RSN
                | DISCONNECT
                | INSTALL_KEY
                | DELETE_KEY
                | REPLACE_GROUP_KEY
        ) && header.request_id != 0
        {
            return error(self, INVALID);
        }
        // Recovery queries must not clear evidence of lost authentication work
        // before the connection has been aborted, even for direct core callers.
        if self.external && self.connection_id != 0 {
            let overflow = self
                .owner
                .and_then(|id| self.sessions.get(&id))
                .is_some_and(|s| s.overflow);
            let fault = self.port.as_ref().is_some_and(|p| p.eapol_state().2);
            if overflow || fault {
                self.invalidate(FAILED, now);
            }
        }
        match command {
            GET_CCMP_STATUS => {
                let Some(port) = &self.port else {
                    return error(self, NOT_SUPPORTED);
                };
                let status = match port.ccmp_status() {
                    Ok(s) => s,
                    Err(e) => return error(self, e),
                };
                return self.output(header, status.bytes().to_vec());
            }
            GET_ASSOCIATION_RSN => {
                if self.association().is_none() {
                    return error(self, BUSY);
                }
                let Some(request) = self.rsn else {
                    return error(self, NOT_SUPPORTED);
                };
                let info = match self
                    .backend
                    .association_rsn(self.device_epoch, self.connection_id)
                {
                    Ok(i) => i,
                    Err(e) => return error(self, e),
                };
                if info.reserved != [0; 2]
                    || info.peer != self.peer
                    || info.own_len != request.ie_len
                    || info.ap_len < 2
                    || info.ap_len > 64
                    || info.own_ie != request.ie
                    || info.ap_ie[info.ap_len as usize..].iter().any(|b| *b != 0)
                {
                    return error(self, FAILED);
                }
                return self.output(header, info.bytes().to_vec());
            }
            GET_SECURITY_STATUS => {
                return self.output(header, self.security_status().bytes().to_vec());
            }
            GET_INFO => return self.output(header, self.name.to_vec()),
            GET_CAPABILITIES => {
                return self.output(header, self.backend.capabilities().bytes().to_vec());
            }
            GET_STATUS | RESYNC => {
                let status = Status {
                    event_sequence: self.sessions.get(&session).unwrap().sequence,
                    ..self.status
                };
                let mut reply = self.output(header, status.bytes().to_vec());
                reply.resync = command == RESYNC && reply.header.result == OK;
                return reply;
            }
            DRAIN_EVENTS => {
                let s = self.sessions.get(&session).unwrap();
                let event = if s.overflow {
                    Event {
                        sequence: s.sequence,
                        timestamp_ns: now,
                        kind: EVENT_OVERFLOW,
                        link_id: self.link_id,
                        device_epoch: self.device_epoch,
                        connection_id: self.connection_id,
                        state_seq: self.state_seq,
                        ..Event::default()
                    }
                } else if let Some(event) = s.events.front() {
                    *event
                } else {
                    return error(
                        self,
                        if self.status.availability == RADIO_REMOVED {
                            DEVICE_GONE
                        } else {
                            WOULD_BLOCK
                        },
                    );
                };
                let mut reply = self.output(header, event.bytes().to_vec());
                if reply.header.result == OK && !s.overflow {
                    reply.consume = Some(event.sequence);
                }
                return reply;
            }
            GET_REQUEST => {
                let Some(id) = payload.get(..8).and_then(Id::decode) else {
                    return error(self, INVALID);
                };
                let Some(p) = self
                    .sessions
                    .get(&session)
                    .and_then(|s| s.requests.get(&id.id))
                else {
                    return error(self, NOT_FOUND);
                };
                return self.output(header, p.info.bytes().to_vec());
            }
            GET_SCAN_RESULTS => {
                let Some(page) = payload.get(..16).and_then(ScanPage::decode) else {
                    return error(self, INVALID);
                };
                if page.reserved != 0 {
                    return error(self, INVALID);
                }
                let Some(scan) = self.scans.get(&page.scan_id).filter(|s| s.expires_ns > now)
                else {
                    return error(self, NOT_FOUND);
                };
                if page.cursor as usize > scan.entries.len() {
                    return error(self, INVALID);
                }
                let mut body = ScanPage {
                    scan_id: page.scan_id,
                    cursor: u32::MAX,
                    reserved: 0,
                }
                .bytes()
                .to_vec();
                if let Some(bss) = scan.entries.get(page.cursor as usize) {
                    body = ScanPage {
                        cursor: page.cursor + 1,
                        ..page
                    }
                    .bytes()
                    .to_vec();
                    body.extend_from_slice(bss.bytes());
                }
                return self.output(header, body);
            }
            CLAIM_CONTROLLER => {
                if !payload.is_empty() {
                    return error(self, INVALID);
                }
                if self.owner.is_some() && self.owner != Some(session) {
                    return error(self, BUSY);
                }
                self.owner = Some(session);
                return error(self, OK);
            }
            SUBSCRIBE_EVENTS => {
                if !payload.is_empty() {
                    return error(self, INVALID);
                }
                self.sessions.get_mut(&session).unwrap().subscribed = true;
                return error(self, OK);
            }
            _ => {}
        }
        if self.owner != Some(session) {
            return error(self, NOT_OWNER);
        }
        match command {
            RELEASE_CONTROLLER => {
                if !payload.is_empty() {
                    return error(self, INVALID);
                }
                self.release(now);
                return error(self, OK);
            }
            ACK_REQUEST | CANCEL_REQUEST | RELEASE_SCAN_RESULTS => {
                let Some(id) = Id::decode(payload) else {
                    return error(self, INVALID);
                };
                if command == RELEASE_SCAN_RESULTS {
                    let result = if self.scans.remove(&id.id).is_some() {
                        OK
                    } else {
                        NOT_FOUND
                    };
                    return error(self, result);
                }
                let Some(p) = self
                    .sessions
                    .get(&session)
                    .and_then(|s| s.requests.get(&id.id))
                else {
                    return error(self, NOT_FOUND);
                };
                let token = p.token;
                if command == ACK_REQUEST {
                    if p.info.result == ACCEPTED {
                        return error(self, BUSY);
                    }
                    self.sessions
                        .get_mut(&session)
                        .unwrap()
                        .requests
                        .remove(&id.id);
                } else if p.info.result == ACCEPTED {
                    if self.key_work.contains_key(&(token.session, token.request)) {
                        self.invalidate(CANCELED, now);
                        return error(self, OK);
                    }
                    self.backend.cancel(token);
                    if self.scan == Some(token) {
                        self.scan = None;
                        self.status.scan_request = 0;
                    }
                    if self.connect == Some(token) || self.disconnect == Some(token) {
                        self.invalidate(CANCELED, now);
                        return error(self, OK);
                    }
                    self.finish(token, CANCELED, 0, now);
                }
                return error(self, OK);
            }
            MOCK_STEP => {
                let Some(step) = MockStep::decode(payload) else {
                    return error(self, INVALID);
                };
                if step.reserved != 0 {
                    return error(self, INVALID);
                }
                let mut step = step;
                if step.token.session == 0 {
                    step.token.session = session;
                }
                let result = self.backend.inject(step).err().unwrap_or(OK);
                return error(self, result);
            }
            MOCK_DATA_TEST => {
                let Some(request) = DataTest::decode(payload) else {
                    return error(self, INVALID);
                };
                let expected = DataTest {
                    operation: request.operation,
                    ..DataTest::default()
                };
                if request != expected || request.operation > DATA_REPLAY_STALE {
                    return error(self, INVALID);
                }
                if self.backend.capabilities().features & (FEATURE_MOCK | FEATURE_DATA)
                    != (FEATURE_MOCK | FEATURE_DATA)
                {
                    return error(self, NOT_SUPPORTED);
                }
                return self.output(header, DataTest::default().bytes().to_vec());
            }
            MOCK_SECURITY_STATS => {
                if self.backend.capabilities().features & FEATURE_MOCK == 0 {
                    return error(self, NOT_SUPPORTED);
                }
                if payload.iter().any(|b| *b != 0) {
                    return error(self, INVALID);
                }
                return match self.backend.security_stats() {
                    Ok(stats) => self.output(header, stats.bytes().to_vec()),
                    Err(code) => error(self, code),
                };
            }
            TX_EAPOL | RX_EAPOL | INSTALL_KEY | DELETE_KEY | REPLACE_GROUP_KEY
            | SET_CONTROLLED_PORT => {
                return self.security_command(session, command, header, payload, now);
            }
            SET_CREDENTIAL => {
                return error(self, NOT_SUPPORTED);
            }
            SCAN | CONNECT | CONNECT_RSN | DISCONNECT => {}
            _ => return error(self, NOT_SUPPORTED),
        }
        let mut connect = Connect::default();
        let mut rsn_request = None;
        let timeout;
        if command == SCAN {
            let Some(scan) = Scan::decode(payload) else {
                return error(self, INVALID);
            };
            if scan.reserved != 0 {
                return error(self, INVALID);
            }
            if self.scan.is_some() || self.connection_id != 0 {
                return error(self, BUSY);
            }
            if self.scans.len() >= MAX_SCANS {
                return error(self, RESOURCE_LIMIT);
            }
            timeout = scan.timeout_ms;
        } else if is_connect(command) {
            let c = if command == CONNECT_RSN {
                let Some(request) = RsnConnect::decode(payload) else {
                    return error(self, INVALID);
                };
                if self.backend.capabilities().features & FEATURE_RSN == 0 {
                    return error(self, NOT_SUPPORTED);
                }
                if request.reserved != 0
                    || request.ie_len != RSN_CCMP_PSK_IE.len() as u32
                    || request.ie[..22] != RSN_CCMP_PSK_IE
                    || request.ie[22..].iter().any(|b| *b != 0)
                    || request.connect.akm != AKM_EXTERNAL
                {
                    return error(self, INVALID);
                }
                rsn_request = Some(request);
                request.connect
            } else {
                let Some(c) = Connect::decode(payload) else {
                    return error(self, INVALID);
                };
                c
            };
            if c.reserved != 0
                || c.ssid_len == 0
                || c.ssid_len > 32
                || c.ssid[c.ssid_len as usize..].iter().any(|b| *b != 0)
            {
                return error(self, INVALID);
            }
            if (c.akm != AKM_OPEN && c.akm != AKM_EXTERNAL)
                || self.backend.capabilities().akm_mask & c.akm == 0
            {
                return error(self, NOT_SUPPORTED);
            }
            if c.akm == AKM_EXTERNAL
                && (self.backend.capabilities().features & FEATURE_AUTH != FEATURE_AUTH
                    || self.port.is_none())
            {
                return error(self, NOT_SUPPORTED);
            }
            if !self.security_clean {
                self.retire_security(now);
                if !self.security_clean {
                    return error(self, FAILED);
                }
            }
            if self.connection_id != 0 || self.scan.is_some() {
                return error(self, BUSY);
            }
            if header.expected_state_seq != self.state_seq {
                return error(self, STATE_CHANGED);
            }
            if header.connection_id != 0 {
                return error(self, STALE_CONNECTION);
            }
            if self.next_connection == u64::MAX {
                return error(self, RESOURCE_LIMIT);
            }
            timeout = c.timeout_ms;
            connect = c;
        } else {
            let Some(s) = Scan::decode(payload) else {
                return error(self, INVALID);
            };
            if s.reserved != 0 {
                return error(self, INVALID);
            }
            if self.connection_id == 0 || header.connection_id != self.connection_id {
                return error(self, STALE_CONNECTION);
            }
            if self.disconnect.is_some() {
                return error(self, BUSY);
            }
            timeout = s.timeout_ms;
        }
        let mut token = match self.reserve(session, header, command, timeout, now) {
            Ok(t) => t,
            Err(e) => return error(self, e),
        };
        if is_connect(command) {
            self.external = connect.akm == AKM_EXTERNAL;
            self.rsn = rsn_request;
            self.peer = [0; 6];
            self.keys.clear();
            self.eapol_seen = self.port.as_ref().map_or(0, |p| p.eapol_state().0);
            self.connection_id = self.next_connection;
            self.next_connection += 1;
            token.connection = self.connection_id;
            let p = self
                .sessions
                .get_mut(&session)
                .unwrap()
                .requests
                .get_mut(&token.request)
                .unwrap();
            p.token = token;
            p.info.connection_id = token.connection;
            self.connect = Some(token);
            self.status.connect_request = token.request;
            self.change_phase(
                if self.backend.capabilities().mlme == MLME_HOST {
                    AUTHENTICATING
                } else {
                    ASSOCIATING
                },
                token,
                now,
            );
        } else if command == SCAN {
            self.scan = Some(token);
            self.status.scan_request = token.request;
        } else {
            let tokens: Vec<Token> = self.key_work.values().map(|w| w.token).collect();
            self.retire_security(now);
            for token in tokens {
                self.finish(token, CANCELED, 0, now);
            }
            if let Some(old) = self.connect.take() {
                self.backend.cancel(old);
                self.finish(old, CANCELED, 0, now);
            }
            self.status.connect_request = 0;
            self.disconnect = Some(token);
            self.status.link_epoch = self.status.link_epoch.saturating_add(1);
            self.change_phase(DISCONNECTING, token, now);
        }
        if let Err(code) = self.backend.submit(
            BackendRequest {
                token,
                command,
                connect,
                rsn: rsn_request,
                key: None,
                key_ref: KeyRef::default(),
            },
            now,
        ) {
            if command == SCAN {
                self.scan = None;
                self.status.scan_request = 0;
            } else {
                self.invalidate(code, now);
            }
            self.finish(token, code, 0, now);
            return error(self, code);
        }
        error(self, ACCEPTED)
    }
    fn security_command(
        &mut self,
        session: u64,
        command: u32,
        header: Header,
        payload: &[u8],
        now: u64,
    ) -> Reply {
        let error = |this: &Self, code| this.reply(header, code, Vec::new());
        if self.backend.capabilities().features & FEATURE_AUTH != FEATURE_AUTH {
            return error(self, NOT_SUPPORTED);
        }
        if self.connection_id == 0 || header.connection_id != self.connection_id {
            return error(self, STALE_CONNECTION);
        }
        let Some(association) = self.association() else {
            return error(self, BUSY);
        };
        let Some(port) = self.port.clone() else {
            return error(self, NOT_SUPPORTED);
        };
        if port.eapol_state().2 {
            self.invalidate(FAILED, now);
            return error(self, FAILED);
        }
        match command {
            RX_EAPOL => {
                if payload.iter().any(|b| *b != 0) {
                    return error(self, INVALID);
                }
                return match port.peek_eapol(association) {
                    Ok(Some(frame)) => {
                        let mut r = self.output(header, frame.bytes().to_vec());
                        if r.header.result == OK {
                            r.eapol_consume = Some((association, frame.sequence));
                        }
                        r
                    }
                    Ok(None) => error(self, WOULD_BLOCK),
                    Err(code) => error(self, code),
                };
            }
            TX_EAPOL => {
                let Some(frame) = Eapol::decode(payload) else {
                    return error(self, INVALID);
                };
                if !valid_eapol_request(&frame, self.peer) {
                    return error(self, INVALID);
                }
                if frame.flags & EAPOL_PROTECTED != 0
                    && self.backend.capabilities().features & FEATURE_PROTECTED_EAPOL == 0
                {
                    return error(self, NOT_SUPPORTED);
                }
                return error(self, port.tx_eapol(association, &frame).err().unwrap_or(OK));
            }
            SET_CONTROLLED_PORT => {
                let Some(request) = PortControl::decode(payload) else {
                    return error(self, INVALID);
                };
                if request.reserved != 0 || request.authorized > 1 {
                    return error(self, INVALID);
                }
                if request.key_revision != self.keys.revision {
                    return error(self, STATE_CHANGED);
                }
                if request.authorized == 1 {
                    if !self.security_clean || !self.key_work.is_empty() || !self.keys.ready() {
                        return error(self, BUSY);
                    }
                    if self.keys.revision == u64::MAX {
                        return error(self, RESOURCE_LIMIT);
                    }
                    if self.status.phase != AUTHORIZED {
                        self.change_phase(
                            AUTHORIZED,
                            Token {
                                session,
                                connection: self.connection_id,
                                ..Token::default()
                            },
                            now,
                        );
                    }
                } else {
                    if let Err(code) = self.block_data(now) {
                        self.invalidate(code, now);
                        return error(self, code);
                    }
                    if let Err(code) = self.keys.advance() {
                        return error(self, code);
                    }
                    self.change_phase(
                        ASSOCIATED,
                        Token {
                            session,
                            connection: self.connection_id,
                            ..Token::default()
                        },
                        now,
                    );
                }
                return error(self, OK);
            }
            _ => {}
        }
        if !self.key_work.is_empty() {
            return error(self, BUSY);
        }
        // Reserve before mutating keys. Validate using an isolated candidate so
        // malformed/stale requests do not consume a ledger entry or close data.
        let mut secret = None;
        let reference;
        let change;
        if matches!(command, INSTALL_KEY | REPLACE_GROUP_KEY) {
            let Some(mut key) = KeyInstall::decode(payload) else {
                return error(self, INVALID);
            };
            if command == REPLACE_GROUP_KEY {
                let result = if self.backend.capabilities().features & FEATURE_GROUP_REKEY == 0 {
                    NOT_SUPPORTED
                } else if key.kind != KEY_GROUP {
                    INVALID
                } else if self.status.phase != AUTHORIZED
                    || !self.keys.ready()
                    || !self.security_clean
                {
                    BUSY
                } else if header.expected_state_seq != self.state_seq {
                    STATE_CHANGED
                } else {
                    OK
                };
                if result != OK {
                    key.key.zeroize();
                    return error(self, result);
                }
            }
            let valid = self.keys.validate_install(&key, self.peer);
            if let Err(code) = valid {
                key.key.zeroize();
                return error(self, code);
            }
            if let Err(code) = self.reserve(session, header, command, 0, now) {
                key.key.zeroize();
                return error(self, code);
            }
            if command != REPLACE_GROUP_KEY {
                if let Err(code) = self.block_data(now) {
                    key.key.zeroize();
                    self.invalidate(code, now);
                    return error(self, code);
                }
            }
            let metadata = match self.keys.prepare_install(&key, self.peer) {
                Ok(k) => k,
                Err(code) => {
                    key.key.zeroize();
                    self.invalidate(code, now);
                    return error(self, code);
                }
            };
            reference = metadata.reference;
            change = if command == REPLACE_GROUP_KEY {
                KeyChange::ReplaceGroup(metadata)
            } else {
                KeyChange::Install(metadata)
            };
            secret = Some(SecretKey(key));
            key.key.zeroize();
        } else {
            let Some(key) = KeyRef::decode(payload) else {
                return error(self, INVALID);
            };
            if !self
                .keys
                .installed
                .get(&(key.kind, key.index))
                .is_some_and(|k| k.reference == key)
            {
                return error(self, NOT_FOUND);
            }
            if let Err(code) = self.reserve(session, header, command, 0, now) {
                return error(self, code);
            }
            if let Err(code) = self.block_data(now) {
                self.invalidate(code, now);
                return error(self, code);
            }
            if let Err(code) = self.keys.prepare_delete(key) {
                self.invalidate(code, now);
                return error(self, code);
            }
            reference = key;
            change = KeyChange::Delete(key);
        }
        let token = Token {
            session,
            request: header.request_id,
            epoch: self.device_epoch,
            connection: self.connection_id,
        };
        self.key_work
            .insert((session, header.request_id), PendingKey { token, change });
        if let Err(code) = self.backend.submit(
            BackendRequest {
                token,
                command,
                connect: Connect::default(),
                rsn: None,
                key: secret,
                key_ref: reference,
            },
            now,
        ) {
            self.invalidate(code, now);
            return error(self, code);
        }
        error(self, ACCEPTED)
    }
    pub fn commit(&mut self, session: u64, reply: &Reply) {
        if self.owner == Some(session) {
            if let (Some(port), Some((association, sequence))) = (&self.port, reply.eapol_consume) {
                port.consume_eapol(association, sequence);
            }
        }
        if let Some(s) = self.sessions.get_mut(&session) {
            if reply.resync {
                s.events.clear();
                s.overflow = false;
            }
            if let Some(sequence) = reply.consume {
                if s.events.front().is_some_and(|e| e.sequence == sequence) {
                    s.events.pop_front();
                }
            }
            s.ready.store(
                s.overflow || !s.events.is_empty() || self.status.availability == RADIO_REMOVED,
                Ordering::Release,
            );
        }
    }
}
