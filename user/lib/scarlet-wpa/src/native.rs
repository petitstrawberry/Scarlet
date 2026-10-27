//! Scarlet Native driver adapter. No Linux sockets, nl80211 or test scripting.
use crate::{Adapter, Error, WPA2_PSK_CCMP_IE};
use scarlet_os::{
    network,
    poll::{self, POLLIN, PollHandle},
    time::monotonic_time_ns,
    wireless::*,
};
use zeroize::Zeroize;

#[derive(Clone, Copy, PartialEq, Eq)]
struct Scope {
    link: u64,
    device: u64,
    connection: u64,
}
impl Scope {
    fn from(h: Header) -> Self {
        Self {
            link: h.link_id,
            device: h.device_epoch,
            connection: h.connection_id,
        }
    }
}
pub struct NativeAdapter {
    session: WirelessSession,
    scope: Scope,
    connect_request: u64,
    versions: [u64; 5],
    installed: [Option<KeyRef>; 5],
    pub own: [u8; 6],
    pub peer: [u8; 6],
    ap_ie: [u8; 64],
    ap_len: usize,
    aborted: bool,
    features: u32,
    authorized: bool,
}
impl NativeAdapter {
    pub fn connect(path: &str, ssid: &[u8], timeout_ms: u32) -> Result<Self, Error> {
        if timeout_ms == 0 {
            return Err(Error::InvalidProfile);
        }
        let mut session = WirelessSession::open(path).map_err(|_| Error::Driver)?;
        let caps = session.capabilities().map_err(|_| Error::Driver)?;
        if caps.features & (FEATURE_AUTH | FEATURE_DATA | FEATURE_RSN)
            != (FEATURE_AUTH | FEATURE_DATA | FEATURE_RSN)
        {
            return Err(Error::Driver);
        }
        session.claim_controller().map_err(|_| Error::Driver)?;
        session.subscribe().map_err(|_| Error::Driver)?;
        let request = session
            .connect_wpa2(ssid, timeout_ms)
            .map_err(|_| Error::Driver)?;
        let identity = session.identity();
        let deadline = monotonic_time_ns().saturating_add(u64::from(timeout_ms) * 1_000_000);
        loop {
            let state = session.status().map_err(|_| Error::Driver)?;
            if session.identity().device_epoch != identity.device_epoch
                || session.identity().connection_id != identity.connection_id
            {
                return Err(Error::ScopeLost);
            }
            if state.phase == ASSOCIATED {
                break;
            }
            if monotonic_time_ns() >= deadline {
                return Err(Error::Timeout);
            }
            let info = session.request(request).map_err(|_| Error::Driver)?;
            if info.result != ACCEPTED && info.result != OK {
                return Err(Error::Driver);
            }
            Self::poll_session(&mut session, deadline.saturating_sub(monotonic_time_ns()))?;
        }
        let scope = Scope::from(session.identity());
        let rsn = session.association_rsn().map_err(|_| Error::Driver)?;
        if Scope::from(session.identity()) != scope
            || rsn.own_len != 22
            || rsn.own_ie[..22] != WPA2_PSK_CCMP_IE
            || rsn.ap_len < 2
            || rsn.ap_len > 64
            || rsn.reserved != [0; 2]
        {
            return Err(Error::Driver);
        }
        let own = network::list_links()
            .map_err(|_| Error::Driver)?
            .into_iter()
            .find(|link| link.id == scope.link)
            .ok_or(Error::ScopeLost)?
            .mac_address;
        Ok(Self {
            session,
            scope,
            connect_request: request,
            versions: [0; 5],
            installed: [None; 5],
            own,
            peer: rsn.peer,
            ap_ie: rsn.ap_ie,
            ap_len: rsn.ap_len as usize,
            aborted: false,
            features: caps.features,
            authorized: false,
        })
    }
    pub fn ap_rsn_ie(&self) -> &[u8] {
        &self.ap_ie[..self.ap_len]
    }
    pub fn session_mut(&mut self) -> &mut WirelessSession {
        &mut self.session
    }
    pub fn connection_id(&self) -> u64 {
        self.scope.connection
    }
    fn drain(session: &mut WirelessSession) -> Result<(), Error> {
        while let Some(event) = session.next_event().map_err(|_| Error::Driver)? {
            if matches!(event.kind, EVENT_OVERFLOW | EVENT_RESET | EVENT_REMOVED) {
                return Err(Error::ScopeLost);
            }
        }
        Ok(())
    }
    fn poll_session(session: &mut WirelessSession, ns: u64) -> Result<(), Error> {
        let mut handles = [PollHandle::new(session.handle().as_raw() as u32, POLLIN)];
        poll::poll(&mut handles, i64::try_from(ns).map_err(|_| Error::Driver)?)
            .map_err(|_| Error::Driver)?;
        Self::drain(session)
    }
    pub fn wait_event(&mut self, ns: u64) -> Result<(), Error> {
        Self::poll_session(&mut self.session, ns)
    }
    pub fn wait_until(&mut self, deadline: Option<u64>) -> Result<(), Error> {
        let timeout = deadline.map_or(-1, |end| {
            end.saturating_sub(monotonic_time_ns()).min(i64::MAX as u64) as i64
        });
        let mut handles = [PollHandle::new(
            self.session.handle().as_raw() as u32,
            POLLIN,
        )];
        poll::poll(&mut handles, timeout).map_err(|_| Error::Driver)?;
        Self::drain(&mut self.session)
    }
    pub fn next_eapol(&mut self) -> Result<Option<Eapol>, Error> {
        self.check()?;
        Self::drain(&mut self.session)?;
        self.check()?;
        let frame = self.session.receive_eapol().map_err(|_| Error::Driver)?;
        if Scope::from(self.session.identity()) != self.scope {
            return Err(Error::ScopeLost);
        }
        Ok(frame)
    }
    fn committed(&mut self, id: u64) -> Result<(), Error> {
        let deadline = monotonic_time_ns().saturating_add(5_000_000_000);
        loop {
            self.check()?;
            let info = self.session.request(id).map_err(|_| Error::Driver)?;
            if Scope::from(self.session.identity()) != self.scope {
                return Err(Error::ScopeLost);
            }
            if info.result != ACCEPTED {
                if info.result != OK {
                    return Err(Error::Driver);
                }
                self.session.acknowledge(id).map_err(|_| Error::Driver)?;
                return Ok(());
            }
            if monotonic_time_ns() >= deadline {
                return Err(Error::Timeout);
            }
            Self::poll_session(
                &mut self.session,
                deadline.saturating_sub(monotonic_time_ns()),
            )?;
        }
    }
    fn slot(kind: u32, index: u32) -> Result<usize, Error> {
        match kind {
            KEY_PAIRWISE if index == 0 => Ok(0),
            KEY_GROUP if index <= 3 => Ok(1 + index as usize),
            _ => Err(Error::Driver),
        }
    }
}
impl Adapter for NativeAdapter {
    fn random(&mut self, output: &mut [u8]) -> Result<(), Error> {
        // Exact-length trusted entropy only. The default syscall/device paths
        // can use a noncryptographic fallback and must not supply WPA nonces.
        let count = unsafe {
            scarlet_sys::syscall3(
                scarlet_sys::Syscall::GetRandom,
                output.as_mut_ptr() as usize,
                output.len(),
                scarlet_abi::GET_RANDOM_FLAG_REQUIRE_ENTROPY,
            )
        };
        if count != output.len() {
            output.zeroize();
            return Err(Error::EntropyUnavailable);
        }
        Ok(())
    }
    fn now_ns(&self) -> u64 {
        monotonic_time_ns()
    }
    fn check(&mut self) -> Result<(), Error> {
        if self.aborted {
            return Err(Error::ScopeLost);
        }
        let status = self.session.status().map_err(|_| Error::Driver)?;
        if Scope::from(self.session.identity()) != self.scope
            || !matches!(status.phase, ASSOCIATED | AUTHORIZED)
        {
            return Err(Error::ScopeLost);
        }
        Ok(())
    }
    fn send(&mut self, peer: [u8; 6], pdu: &[u8]) -> Result<(), Error> {
        self.check()?;
        self.session
            .transmit_eapol_protected(
                peer,
                pdu,
                self.authorized && self.features & FEATURE_PROTECTED_EAPOL != 0,
            )
            .map_err(|_| Error::Driver)
    }
    fn key(
        &mut self,
        kind: u32,
        peer: [u8; 6],
        index: u32,
        sequence: &[u8],
        key: &[u8],
    ) -> Result<(), Error> {
        self.check()?;
        if kind == 0 {
            let slot = Self::slot(
                if peer == [255; 6] {
                    KEY_GROUP
                } else {
                    KEY_PAIRWISE
                },
                index,
            )?;
            if let Some(reference) = self.installed[slot] {
                let id = self
                    .session
                    .delete_key(reference)
                    .map_err(|_| Error::Driver)?;
                self.committed(id)?;
                self.installed[slot] = None;
            }
            return Ok(());
        }
        if key.len() != 16 || sequence.len() > 6 {
            return Err(Error::Driver);
        }
        let slot = Self::slot(kind, index)?;
        let version = self.versions[slot].checked_add(1).ok_or(Error::Driver)?;
        self.versions[slot] = version;
        let mut request = KeyInstall {
            version,
            kind,
            index,
            cipher: CIPHER_CCMP128,
            key_len: 16,
            peer,
            ..KeyInstall::default()
        };
        request.key[..16].copy_from_slice(key);
        request.rx_sequence[..sequence.len()].copy_from_slice(sequence);
        let rotating = kind == KEY_GROUP && self.authorized;
        if rotating && self.features & FEATURE_GROUP_REKEY == 0 {
            request.key.zeroize();
            return Err(Error::Driver);
        }
        let submitted = if rotating {
            self.session.replace_group_key(&request)
        } else {
            self.session.install_key(&request)
        };
        request.key.zeroize();
        let id = submitted.map_err(|_| Error::Driver)?;
        self.committed(id)?;
        if rotating {
            self.installed[1..].fill(None);
        }
        self.installed[slot] = Some(KeyRef {
            version,
            kind,
            index,
        });
        Ok(())
    }
    fn set_authorized(&mut self, authorized: bool) -> Result<(), Error> {
        self.check()?;
        let state = self.session.security_status().map_err(|_| Error::Driver)?;
        if Scope::from(self.session.identity()) != self.scope {
            return Err(Error::ScopeLost);
        }
        self.session
            .set_controlled_port(authorized, state.key_revision)
            .map_err(|_| Error::Driver)?;
        self.authorized = authorized;
        if authorized && self.connect_request != 0 {
            let id = self.connect_request;
            self.committed(id)?;
            self.connect_request = 0;
        }
        Ok(())
    }
    fn require_protected_group(&self) -> bool {
        self.features & FEATURE_PROTECTED_EAPOL != 0
    }
    fn abort(&mut self) {
        if self.aborted {
            return;
        }
        // Never adopt a refreshed query identity and then tear down a new scope.
        if self.session.status().is_ok() && Scope::from(self.session.identity()) == self.scope {
            if let Ok(state) = self.session.security_status() {
                if Scope::from(self.session.identity()) == self.scope {
                    let _ = self.session.set_controlled_port(false, state.key_revision);
                }
            }
            if Scope::from(self.session.identity()) == self.scope {
                let _ = self.session.disconnect(1000);
            }
        }
        self.aborted = true;
        self.authorized = false;
        self.installed.fill(None);
    }
}
impl Drop for NativeAdapter {
    fn drop(&mut self) {
        self.abort();
    }
}
