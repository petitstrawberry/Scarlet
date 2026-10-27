//! Deterministic control/data/authentication backend. Keys model firmware slots;
//! optional CCMP mode commits software data keys through the authentication port.
//! It does not emulate RF, firmware or physical DMA.

extern crate alloc;
use super::{
    auth::{Association, AuthenticationPort, SecretKey},
    core::*,
};
use alloc::{
    collections::{BTreeMap, VecDeque},
    sync::Arc,
    vec,
};
use scarlet_abi::wireless::*;

pub struct MockBackend {
    mlme: u32,
    automatic: bool,
    data: bool,
    auth: bool,
    wpa2: bool,
    ccmp: bool,
    port: Option<Arc<dyn AuthenticationPort>>,
    rsn: Option<AssociationRsn>,
    scope: Option<(u64, u64)>,
    key_jobs: BTreeMap<(u64, u64), BackendRequest>,
    keys: BTreeMap<(u32, u32), SecretKey>,
    retired: u64,
    retire_failure: bool,
    queue: VecDeque<(u64, BackendEvent)>,
}
impl MockBackend {
    pub fn new(mlme: u32, automatic: bool) -> Self {
        Self {
            mlme,
            automatic,
            data: false,
            auth: false,
            wpa2: false,
            ccmp: false,
            port: None,
            rsn: None,
            scope: None,
            key_jobs: BTreeMap::new(),
            keys: BTreeMap::new(),
            retired: 0,
            retire_failure: false,
            queue: VecDeque::new(),
        }
    }
    pub fn with_data(mut self) -> Self {
        self.data = true;
        self
    }
    pub fn with_authentication(mut self) -> Self {
        self.data = true;
        self.auth = true;
        self
    }
    pub fn with_wpa2(mut self) -> Self {
        self = self.with_authentication();
        self.wpa2 = true;
        self
    }
    pub fn with_ccmp(mut self) -> Self {
        self = self.with_wpa2();
        self.ccmp = true;
        self
    }
    fn completed(token: Token, result: i32, now: u64) -> BackendEvent {
        let mut bss = Bss {
            ssid_len: 11,
            bssid: [2, 0, 0, 0, 0, 1],
            frequency_mhz: 2412,
            signal_mbm: -4200,
            akm: AKM_OPEN,
            seen_ns: now,
            ..Bss::default()
        };
        bss.ssid[..11].copy_from_slice(b"ScarletMock");
        BackendEvent {
            token,
            kind: BACKEND_COMPLETE,
            result,
            bss: vec![bss],
        }
    }
}
impl WirelessBackend for MockBackend {
    fn next_deadline_ns(&self) -> Option<u64> {
        self.queue.iter().map(|(at, _)| *at).min()
    }
    fn attach_authentication_port(&mut self, port: Arc<dyn AuthenticationPort>) {
        self.port = Some(port);
    }
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            mlme: self.mlme,
            akm_mask: if self.ccmp {
                AKM_EXTERNAL
            } else {
                AKM_OPEN | if self.auth { AKM_EXTERNAL } else { 0 }
            },
            features: FEATURE_MOCK
                | if self.data { FEATURE_DATA } else { 0 }
                | if self.auth { FEATURE_AUTH } else { 0 }
                | if self.wpa2 { FEATURE_RSN } else { 0 }
                | if self.ccmp {
                    FEATURE_CCMP | FEATURE_PROTECTED_EAPOL | FEATURE_GROUP_REKEY
                } else {
                    0
                },
            max_sessions: MAX_SESSIONS as u32,
            max_requests: MAX_REQUESTS as u32,
            max_events: MAX_EVENTS as u32,
            max_timeout_ms: MAX_TIMEOUT_MS,
            reserved: 0,
        }
    }
    fn submit(&mut self, request: BackendRequest, now: u64) -> Result<(), i32> {
        if matches!(
            request.command,
            INSTALL_KEY | DELETE_KEY | REPLACE_GROUP_KEY
        ) {
            if !self.auth {
                return Err(NOT_SUPPORTED);
            }
            if self.scope != Some((request.token.epoch, request.token.connection)) {
                return Err(STALE_CONNECTION);
            }
            if self.key_jobs.len() == MAX_REQUESTS {
                return Err(RESOURCE_LIMIT);
            }
            let token = request.token;
            let rekey = request.command == REPLACE_GROUP_KEY;
            if self.wpa2 && self.automatic && self.queue.len() == MAX_REQUESTS {
                return Err(RESOURCE_LIMIT);
            }
            self.key_jobs
                .insert((token.session, token.request), request);
            if self.wpa2 && self.automatic {
                // A deliberately visible mock GTK confirmation window lets the
                // integration test exchange old-key data while work is pending.
                self.queue.push_back((
                    now.saturating_add(if rekey { 1_000_000_000 } else { 5_000_000 }),
                    Self::completed(token, OK, now),
                ));
            }
            return Ok(()); // Explicit firmware confirmation via MockStep.
        }
        if matches!(request.command, CONNECT | CONNECT_RSN) {
            if request.command == CONNECT_RSN {
                if !self.wpa2 {
                    return Err(NOT_SUPPORTED);
                }
                let Some(config) = request.rsn else {
                    return Err(INVALID);
                };
                let mut info = AssociationRsn {
                    own_len: config.ie_len,
                    ap_len: 22,
                    peer: [2, 0, 0, 0, 0, 1],
                    own_ie: config.ie,
                    ..AssociationRsn::default()
                };
                info.ap_ie[..22].copy_from_slice(&RSN_CCMP_PSK_IE);
                self.rsn = Some(info);
            } else {
                self.rsn = None;
            }
            self.scope = Some((request.token.epoch, request.token.connection));
        }
        if !self.automatic {
            return Ok(());
        }
        if self.queue.len() + 2 > MAX_REQUESTS {
            return Err(RESOURCE_LIMIT);
        }
        if matches!(request.command, CONNECT | CONNECT_RSN) && (self.mlme == MLME_HOST || self.data)
        {
            self.queue.push_back((
                now.saturating_add(25_000_000),
                BackendEvent {
                    token: request.token,
                    kind: BACKEND_ASSOCIATED,
                    result: OK,
                    bss: if self.auth {
                        Self::completed(request.token, OK, now).bss
                    } else {
                        vec![]
                    },
                },
            ));
        }
        if matches!(request.command, CONNECT | CONNECT_RSN) && self.data {
            return Ok(());
        }
        self.queue.push_back((
            now.saturating_add(50_000_000),
            Self::completed(request.token, OK, now),
        ));
        Ok(())
    }
    fn cancel(&mut self, token: Token) {
        self.queue.retain(|(_, event)| event.token != token);
        self.key_jobs.remove(&(token.session, token.request));
    }
    fn retire_security(&mut self) -> Result<(), i32> {
        if !self.auth {
            return Err(NOT_SUPPORTED);
        }
        if self.retire_failure {
            return Err(FAILED);
        }
        if self.ccmp {
            self.port.as_ref().ok_or(FAILED)?.retire_ccmp()?;
        }
        self.retired += self.keys.len() as u64;
        self.keys.clear();
        self.key_jobs.clear();
        self.scope = None;
        self.rsn = None;
        Ok(())
    }
    fn association_rsn(&self, device: u64, connection: u64) -> Result<AssociationRsn, i32> {
        if self.scope != Some((device, connection)) {
            return Err(STALE_CONNECTION);
        }
        self.rsn.ok_or(NOT_SUPPORTED)
    }
    fn security_stats(&self) -> Result<MockSecurityStats, i32> {
        if !self.auth {
            return Err(NOT_SUPPORTED);
        }
        Ok(MockSecurityStats {
            installed: self.keys.len() as u32,
            pending: self.key_jobs.len() as u32,
            retired: self.retired,
        })
    }
    fn poll(&mut self, now: u64) -> Option<BackendEvent> {
        let index = self.queue.iter().position(|(at, _)| *at <= now)?;
        let mut event = self.queue.remove(index)?.1;
        if event.kind == BACKEND_COMPLETE {
            let id = (event.token.session, event.token.request);
            if self
                .key_jobs
                .get(&id)
                .is_some_and(|r| r.token == event.token)
            {
                let mut job = self.key_jobs.remove(&id).unwrap();
                if event.result == OK
                    && self.scope == Some((event.token.epoch, event.token.connection))
                {
                    let slot = (job.key_ref.kind, job.key_ref.index);
                    if matches!(job.command, INSTALL_KEY | REPLACE_GROUP_KEY) {
                        if let Some(key) = job.key.take() {
                            let result = if self.ccmp {
                                self.port.as_ref().ok_or(FAILED).and_then(|p| {
                                    let scope = Association {
                                        device: event.token.epoch,
                                        connection: event.token.connection,
                                        peer: self.rsn.map_or([2, 0, 0, 0, 0, 1], |i| i.peer),
                                    };
                                    if job.command == REPLACE_GROUP_KEY {
                                        p.replace_group_ccmp(scope, &key.0)
                                    } else {
                                        p.install_ccmp(scope, &key.0)
                                    }
                                })
                            } else {
                                Ok(())
                            };
                            // GTK peer is broadcast on the wire, whereas the
                            // association identity always names the AP.
                            if let Err(e) = result {
                                event.result = e;
                            } else {
                                if job.command == REPLACE_GROUP_KEY {
                                    let before = self.keys.len();
                                    self.keys.retain(|(kind, _), _| *kind != KEY_GROUP);
                                    self.retired += (before - self.keys.len()) as u64;
                                }
                                self.keys.insert(slot, key);
                            }
                        }
                    } else {
                        let result = if self.ccmp {
                            self.port.as_ref().ok_or(FAILED).and_then(|p| {
                                p.delete_ccmp(
                                    Association {
                                        device: event.token.epoch,
                                        connection: event.token.connection,
                                        peer: self.rsn.map_or([2, 0, 0, 0, 0, 1], |i| i.peer),
                                    },
                                    job.key_ref,
                                )
                            })
                        } else {
                            Ok(())
                        };
                        if let Err(e) = result {
                            event.result = e;
                        } else {
                            self.keys.remove(&slot);
                            self.retired += 1;
                        }
                    }
                }
            }
        }
        Some(event)
    }
    fn inject(&mut self, step: MockStep) -> Result<(), i32> {
        if step.step == STEP_RETIRE_FAILURE {
            self.retire_failure = true;
            return Ok(());
        }
        if step.step == STEP_RETIRE_RECOVER {
            self.retire_failure = false;
            return Ok(());
        }
        if self.queue.len() >= MAX_REQUESTS {
            return Err(RESOURCE_LIMIT);
        }
        let kind = match step.step {
            STEP_SUCCESS | STEP_FAILURE => BACKEND_COMPLETE,
            STEP_ASSOCIATED => BACKEND_ASSOCIATED,
            STEP_RESET => BACKEND_RESET,
            STEP_REMOVE => BACKEND_REMOVED,
            _ => return Err(INVALID),
        };
        let mut event = Self::completed(
            step.token,
            if step.step == STEP_FAILURE {
                FAILED
            } else {
                OK
            },
            0,
        );
        event.kind = kind;
        self.queue.push_front((0, event));
        Ok(())
    }
}
