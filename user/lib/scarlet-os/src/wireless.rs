//! Owned native Wi-Fi control session and bounded inline command transport.
//! Capabilities distinguish control-only mocks from controlled data transports.

extern crate alloc;
use crate::handle::{Handle, HandleError};
use alloc::{vec, vec::Vec};
pub use scarlet_abi::wireless::*;

#[derive(Debug, PartialEq, Eq)]
pub enum WirelessError {
    Handle(HandleError),
    Operation(i32),
    InvalidResponse,
    InvalidArgument,
    Indeterminate {
        request_id: u64,
        source: HandleError,
    },
}
impl From<HandleError> for WirelessError {
    fn from(error: HandleError) -> Self {
        Self::Handle(error)
    }
}
// Wipe request storage on every exit, including a failed syscall/copyout.
struct CommandBuffer(Vec<u8>);
impl core::ops::Deref for CommandBuffer {
    type Target = Vec<u8>;
    fn deref(&self) -> &Vec<u8> {
        &self.0
    }
}
impl core::ops::DerefMut for CommandBuffer {
    fn deref_mut(&mut self) -> &mut Vec<u8> {
        &mut self.0
    }
}
impl Drop for CommandBuffer {
    fn drop(&mut self) {
        for byte in &mut self.0 {
            unsafe {
                core::ptr::write_volatile(byte, 0);
            }
        }
    }
}
pub struct Response {
    pub header: Header,
    pub payload: Vec<u8>,
}

/// One owning endpoint. Duplicated handles share a kernel session; submit IDs
/// must be allocated by one sequencer, so this wrapper is intentionally not Clone.
pub struct WirelessSession {
    handle: Handle,
    snapshot: Header,
    next_request: u64,
}
impl WirelessSession {
    pub fn open(path: &str) -> Result<Self, WirelessError> {
        Self::from_handle(Handle::open(path, 2)?)
    }
    pub fn from_handle(handle: Handle) -> Result<Self, WirelessError> {
        let mut session = Self {
            handle,
            snapshot: Header::new(HEADER_SIZE),
            next_request: 1,
        };
        session.call(GET_INFO, &[], 32, 0)?;
        Ok(session)
    }
    pub fn handle(&self) -> &Handle {
        &self.handle
    }
    pub fn identity(&self) -> Header {
        self.snapshot
    }

    /// Safe raw-protocol exchange: capacity is payload capacity, all pointers
    /// remain private to this synchronous method. Operation errors stay in the
    /// response header so callers can inspect required lengths and new epochs.
    pub fn exchange(
        &mut self,
        command: u32,
        payload: &[u8],
        output_capacity: usize,
        request_id: u64,
    ) -> Result<Response, WirelessError> {
        let payload_capacity = payload.len().max(output_capacity);
        let capacity = HEADER_SIZE
            .checked_add(payload_capacity)
            .filter(|n| *n <= MAX_BUFFER)
            .ok_or(WirelessError::InvalidArgument)?;
        let mut buffer = CommandBuffer(vec![0; capacity]);
        let mut header = self.snapshot;
        header.abi_version = ABI_VERSION;
        header.header_len = HEADER_SIZE as u16;
        header.buffer_len = capacity as u32;
        header.request_id = request_id;
        header.result = 0;
        header.flags = 0;
        header.reserved = 0;
        if matches!(
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
        ) {
            header.device_epoch = 0;
        }
        buffer[..HEADER_SIZE].copy_from_slice(header.bytes());
        buffer[HEADER_SIZE..HEADER_SIZE + payload.len()].copy_from_slice(payload);
        // SAFETY: the exclusively borrowed, initialized inline buffer remains
        // writable for its validated capacity throughout the synchronous call.
        if let Err(source) = unsafe { self.handle.control(command, buffer.as_mut_ptr() as usize) } {
            return Err(if request_id == 0 {
                WirelessError::Handle(source)
            } else {
                WirelessError::Indeterminate { request_id, source }
            });
        }
        let response =
            Header::decode(&buffer[..HEADER_SIZE]).ok_or(WirelessError::InvalidResponse)?;
        if response.abi_version != ABI_VERSION
            || response.header_len as usize != HEADER_SIZE
            || response.flags != 0
            || response.reserved != 0
            || response.request_id != request_id
            || !(HEADER_SIZE..=MAX_BUFFER).contains(&(response.buffer_len as usize))
        {
            return Err(WirelessError::InvalidResponse);
        }
        self.snapshot = response;
        self.snapshot.request_id = 0;
        self.snapshot.result = 0;
        if response.result == BUFFER_TOO_SMALL {
            return Ok(Response {
                header: response,
                payload: Vec::new(),
            });
        }
        if response.buffer_len as usize > capacity {
            return Err(WirelessError::InvalidResponse);
        }
        buffer.truncate(response.buffer_len as usize);
        Ok(Response {
            header: response,
            payload: buffer[HEADER_SIZE..].to_vec(),
        })
    }
    fn call(
        &mut self,
        command: u32,
        payload: &[u8],
        capacity: usize,
        request: u64,
    ) -> Result<Response, WirelessError> {
        let response = self.exchange(command, payload, capacity, request)?;
        if !matches!(response.header.result, OK | ACCEPTED) {
            return Err(WirelessError::Operation(response.header.result));
        }
        Ok(response)
    }
    fn query<T: Wire>(&mut self, command: u32, payload: &[u8]) -> Result<T, WirelessError> {
        let response = self.call(command, payload, core::mem::size_of::<T>(), 0)?;
        T::decode(&response.payload).ok_or(WirelessError::InvalidResponse)
    }
    fn submit(&mut self, command: u32, payload: &[u8]) -> Result<u64, WirelessError> {
        let id = self.next_request;
        self.next_request = id.checked_add(1).ok_or(WirelessError::InvalidArgument)?;
        self.call(command, payload, 0, id)?;
        Ok(id)
    }
    pub fn capabilities(&mut self) -> Result<Capabilities, WirelessError> {
        self.query(GET_CAPABILITIES, &[])
    }
    pub fn status(&mut self) -> Result<Status, WirelessError> {
        self.query(GET_STATUS, &[])
    }
    pub fn claim_controller(&mut self) -> Result<(), WirelessError> {
        self.call(CLAIM_CONTROLLER, &[], 0, 0).map(|_| ())
    }
    pub fn release_controller(&mut self) -> Result<(), WirelessError> {
        self.call(RELEASE_CONTROLLER, &[], 0, 0).map(|_| ())
    }
    pub fn subscribe(&mut self) -> Result<(), WirelessError> {
        self.call(SUBSCRIBE_EVENTS, &[], 0, 0).map(|_| ())
    }
    pub fn resync(&mut self) -> Result<Status, WirelessError> {
        self.query(RESYNC, &[])
    }
    pub fn next_event(&mut self) -> Result<Option<Event>, WirelessError> {
        let response = self.exchange(DRAIN_EVENTS, &[], core::mem::size_of::<Event>(), 0)?;
        if response.header.result == WOULD_BLOCK {
            return Ok(None);
        }
        if response.header.result != OK {
            return Err(WirelessError::Operation(response.header.result));
        }
        let event = Event::decode(&response.payload).ok_or(WirelessError::InvalidResponse)?;
        if event.reserved != 0 || event.reserved2 != 0 {
            return Err(WirelessError::InvalidResponse);
        }
        Ok(Some(event))
    }
    pub fn scan(&mut self, timeout_ms: u32) -> Result<u64, WirelessError> {
        self.submit(
            SCAN,
            Scan {
                timeout_ms,
                reserved: 0,
            }
            .bytes(),
        )
    }
    pub fn connect_open(&mut self, ssid: &[u8], timeout_ms: u32) -> Result<u64, WirelessError> {
        self.connect_with_akm(ssid, timeout_ms, AKM_OPEN)
    }
    pub fn connect_external(&mut self, ssid: &[u8], timeout_ms: u32) -> Result<u64, WirelessError> {
        self.connect_with_akm(ssid, timeout_ms, AKM_EXTERNAL)
    }
    fn connect_with_akm(
        &mut self,
        ssid: &[u8],
        timeout_ms: u32,
        akm: u32,
    ) -> Result<u64, WirelessError> {
        if ssid.is_empty() || ssid.len() > 32 {
            return Err(WirelessError::InvalidArgument);
        }
        let mut request = Connect {
            ssid_len: ssid.len() as u32,
            akm,
            timeout_ms,
            ..Connect::default()
        };
        request.ssid[..ssid.len()].copy_from_slice(ssid);
        self.submit(CONNECT, request.bytes())
    }
    pub fn disconnect(&mut self, timeout_ms: u32) -> Result<u64, WirelessError> {
        self.submit(
            DISCONNECT,
            Scan {
                timeout_ms,
                reserved: 0,
            }
            .bytes(),
        )
    }
    pub fn request(&mut self, id: u64) -> Result<RequestInfo, WirelessError> {
        self.query(GET_REQUEST, Id { id }.bytes())
    }
    pub fn acknowledge(&mut self, id: u64) -> Result<(), WirelessError> {
        self.call(ACK_REQUEST, Id { id }.bytes(), 0, 0).map(|_| ())
    }
    pub fn cancel(&mut self, id: u64) -> Result<(), WirelessError> {
        self.call(CANCEL_REQUEST, Id { id }.bytes(), 0, 0)
            .map(|_| ())
    }
    pub fn release_scan(&mut self, id: u64) -> Result<(), WirelessError> {
        self.call(RELEASE_SCAN_RESULTS, Id { id }.bytes(), 0, 0)
            .map(|_| ())
    }
    pub fn scan_result(
        &mut self,
        scan_id: u64,
        cursor: u32,
    ) -> Result<(Option<Bss>, u32), WirelessError> {
        let response = self.call(
            GET_SCAN_RESULTS,
            ScanPage {
                scan_id,
                cursor,
                reserved: 0,
            }
            .bytes(),
            core::mem::size_of::<ScanPage>() + core::mem::size_of::<Bss>(),
            0,
        )?;
        let page = response
            .payload
            .get(..16)
            .and_then(ScanPage::decode)
            .ok_or(WirelessError::InvalidResponse)?;
        if page.scan_id != scan_id || page.reserved != 0 {
            return Err(WirelessError::InvalidResponse);
        }
        if page.cursor == u32::MAX && response.payload.len() == 16 {
            return Ok((None, page.cursor));
        }
        let bss = response
            .payload
            .get(16..)
            .and_then(Bss::decode)
            .ok_or(WirelessError::InvalidResponse)?;
        if bss.ssid_len > 32 || bss.reserved != 0 || bss.reserved2 != 0 {
            return Err(WirelessError::InvalidResponse);
        }
        Ok((Some(bss), page.cursor))
    }
    /// Associate with an explicit WPA2-Personal/CCMP RSN IE; no credentials enter the kernel.
    pub fn connect_wpa2(&mut self, ssid: &[u8], timeout_ms: u32) -> Result<u64, WirelessError> {
        if ssid.is_empty() || ssid.len() > 32 {
            return Err(WirelessError::InvalidArgument);
        }
        let mut request = RsnConnect::default();
        request.connect.ssid[..ssid.len()].copy_from_slice(ssid);
        request.connect.ssid_len = ssid.len() as u32;
        request.connect.akm = AKM_EXTERNAL;
        request.connect.timeout_ms = timeout_ms;
        request.ie_len = 22;
        request.ie[..22].copy_from_slice(&RSN_CCMP_PSK_IE);
        self.submit(CONNECT_RSN, request.bytes())
    }
    pub fn association_rsn(&mut self) -> Result<AssociationRsn, WirelessError> {
        self.query(GET_ASSOCIATION_RSN, &[])
    }
    pub fn ccmp_status(&mut self) -> Result<CcmpStatus, WirelessError> {
        self.query(GET_CCMP_STATUS, &[])
    }
    pub fn security_status(&mut self) -> Result<SecurityStatus, WirelessError> {
        self.query(GET_SECURITY_STATUS, &[])
    }
    pub fn install_key(&mut self, key: &KeyInstall) -> Result<u64, WirelessError> {
        self.submit(INSTALL_KEY, key.bytes())
    }
    /// Confirmed atomic GTK cutover on an already-authorized connection.
    pub fn replace_group_key(&mut self, key: &KeyInstall) -> Result<u64, WirelessError> {
        self.submit(REPLACE_GROUP_KEY, key.bytes())
    }
    pub fn delete_key(&mut self, key: KeyRef) -> Result<u64, WirelessError> {
        self.submit(DELETE_KEY, key.bytes())
    }
    pub fn set_controlled_port(
        &mut self,
        authorized: bool,
        key_revision: u64,
    ) -> Result<(), WirelessError> {
        self.call(
            SET_CONTROLLED_PORT,
            PortControl {
                key_revision,
                authorized: authorized as u32,
                reserved: 0,
            }
            .bytes(),
            0,
            0,
        )
        .map(|_| ())
    }
    /// Success means handed to the transport, not an over-the-air TX ACK.
    pub fn transmit_eapol(&mut self, peer: [u8; 6], pdu: &[u8]) -> Result<(), WirelessError> {
        self.transmit_eapol_protected(peer, pdu, false)
    }
    pub fn transmit_eapol_protected(
        &mut self,
        peer: [u8; 6],
        pdu: &[u8],
        protected: bool,
    ) -> Result<(), WirelessError> {
        if pdu.len() > MAX_EAPOL {
            return Err(WirelessError::InvalidArgument);
        }
        let mut frame = Eapol {
            peer,
            length: pdu.len() as u32,
            flags: if protected { EAPOL_PROTECTED } else { 0 },
            ..Eapol::default()
        };
        frame.payload[..pdu.len()].copy_from_slice(pdu);
        self.call(TX_EAPOL, frame.bytes(), 0, 0).map(|_| ())
    }
    pub fn receive_eapol(&mut self) -> Result<Option<Eapol>, WirelessError> {
        let response = self.exchange(RX_EAPOL, &[], core::mem::size_of::<Eapol>(), 0)?;
        if response.header.result == WOULD_BLOCK {
            return Ok(None);
        }
        if response.header.result != OK {
            return Err(WirelessError::Operation(response.header.result));
        }
        let frame = Eapol::decode(&response.payload).ok_or(WirelessError::InvalidResponse)?;
        if frame.length as usize > MAX_EAPOL
            || frame.reserved != 0
            || frame.flags & !EAPOL_PROTECTED != 0
        {
            return Err(WirelessError::InvalidResponse);
        }
        Ok(Some(frame))
    }
    pub fn mock_security_stats(&mut self) -> Result<MockSecurityStats, WirelessError> {
        self.query(MOCK_SECURITY_STATS, &[])
    }
    /// Control/query held queues on an explicitly enabled data-capable mock.
    pub fn data_test(&mut self, operation: u32) -> Result<DataTest, WirelessError> {
        self.query(
            MOCK_DATA_TEST,
            DataTest {
                operation,
                ..DataTest::default()
            }
            .bytes(),
        )
    }

    /// Script a test radio. A production backend returns NOT_SUPPORTED.
    pub fn mock_step(
        &mut self,
        request: u64,
        connection: u64,
        epoch: u64,
        step: u32,
    ) -> Result<(), WirelessError> {
        self.call(
            MOCK_STEP,
            MockStep {
                token: Token {
                    session: 0,
                    request,
                    epoch,
                    connection,
                },
                step,
                reserved: 0,
            }
            .bytes(),
            0,
            0,
        )
        .map(|_| ())
    }
}
