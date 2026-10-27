//! Authentication lifetime helpers; no cryptographic handshake or encryption.
extern crate alloc;
use alloc::{
    collections::{BTreeMap, VecDeque},
    vec::Vec,
};
use scarlet_abi::wireless::*;
use sha2::{Digest, Sha256};
use zeroize::Zeroize;

/// A queue-publication notification. Implementations must not block or call
/// back into Radio; callers may hold a backend or authentication inbox lock.
pub trait WirelessNotifier: Send + Sync {
    fn notify(&self);
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Association {
    pub device: u64,
    pub connection: u64,
    pub peer: [u8; 6],
}
/// No callback into Radio; called in task context with the radio mutex held.
pub trait AuthenticationPort: Send + Sync {
    /// Close and quiesce ordinary data before changing or retiring any key.
    fn block(&self) -> Result<(), i32>;
    /// Optional software CCMP data backend. Installation follows confirmed key work.
    fn install_ccmp(&self, _scope: Association, _key: &KeyInstall) -> Result<(), i32> {
        Err(NOT_SUPPORTED)
    }
    /// Atomic GTK cutover with PTK, packet tokens and PN preserved.
    fn replace_group_ccmp(&self, _scope: Association, _key: &KeyInstall) -> Result<(), i32> {
        Err(NOT_SUPPORTED)
    }
    fn delete_ccmp(&self, _scope: Association, _key: KeyRef) -> Result<(), i32> {
        Err(NOT_SUPPORTED)
    }
    fn retire_ccmp(&self) -> Result<(), i32> {
        Err(NOT_SUPPORTED)
    }
    fn ccmp_status(&self) -> Result<CcmpStatus, i32> {
        Err(NOT_SUPPORTED)
    }
    /// Deliver one transport-received Ethernet frame through the controlled
    /// facade. The facade classifies EAPOL, decrypts, and stamps the link token.
    fn receive_frame(&self, _packet: &crate::device::network::DevicePacket) -> Result<(), i32> {
        Err(NOT_SUPPORTED)
    }
    fn tx_eapol(&self, association: Association, frame: &Eapol) -> Result<(), i32>;
    fn peek_eapol(&self, association: Association) -> Result<Option<Eapol>, i32>;
    fn consume_eapol(&self, association: Association, sequence: u64);
    fn eapol_state(&self) -> (u64, usize, bool);
}
/// The sole owning backend copy. Neither this type nor the wire key implements Debug.
pub struct SecretKey(pub KeyInstall);
impl Drop for SecretKey {
    fn drop(&mut self) {
        self.0.key.zeroize();
        self.0.rx_sequence.zeroize();
    }
}

pub fn valid_peer(peer: [u8; 6]) -> bool {
    peer != [0; 6] && peer[0] & 1 == 0
}
pub fn valid_eapol(pdu: &[u8]) -> bool {
    pdu.len() >= 4
        && pdu.len() <= MAX_EAPOL
        && (1..=3).contains(&pdu[0])
        && usize::from(u16::from_be_bytes([pdu[2], pdu[3]])) + 4 == pdu.len()
}
pub fn valid_eapol_request(frame: &Eapol, peer: [u8; 6]) -> bool {
    let len = frame.length as usize;
    len <= MAX_EAPOL
        && frame.sequence == 0
        && frame.flags & !EAPOL_PROTECTED == 0
        && frame.reserved == 0
        && frame.peer == peer
        && valid_eapol(&frame.payload[..len])
        && frame.payload[len..].iter().all(|b| *b == 0)
}
pub struct EapolInbox {
    queue: VecDeque<Eapol>,
    pub sequence: u64,
    pub fault: bool,
}
impl Default for EapolInbox {
    fn default() -> Self {
        Self {
            queue: VecDeque::with_capacity(MAX_EAPOL_QUEUE),
            sequence: 0,
            fault: false,
        }
    }
}
impl EapolInbox {
    pub fn clear(&mut self) {
        for p in &mut self.queue {
            p.payload.zeroize();
        }
        self.queue.clear();
        self.fault = false;
    }
    pub fn push(&mut self, mut frame: Eapol) {
        if self.fault {
            return;
        }
        if self.queue.len() == MAX_EAPOL_QUEUE || self.sequence == u64::MAX {
            self.fault = true;
            return;
        }
        self.sequence += 1;
        frame.sequence = self.sequence;
        self.queue.push_back(frame);
    }
    pub fn peek(&self) -> Option<Eapol> {
        self.queue.front().copied()
    }
    pub fn consume(&mut self, sequence: u64) {
        if self.queue.front().is_some_and(|f| f.sequence == sequence) {
            if let Some(mut frame) = self.queue.pop_front() {
                frame.payload.zeroize();
            }
        }
    }
    pub fn len(&self) -> usize {
        self.queue.len()
    }
}
impl Drop for EapolInbox {
    fn drop(&mut self) {
        self.clear();
    }
}

#[derive(Clone, Copy)]
pub struct KeyMetadata {
    pub reference: KeyRef,
}
pub struct KeyState {
    pub revision: u64,
    pub installed: BTreeMap<(u32, u32), KeyMetadata>,
    watermarks: BTreeMap<(u32, u32), u64>,
    fingerprints: Vec<[u8; 32]>,
}
impl Default for KeyState {
    fn default() -> Self {
        Self {
            revision: 1,
            installed: BTreeMap::new(),
            watermarks: BTreeMap::new(),
            fingerprints: Vec::new(),
        }
    }
}
impl KeyState {
    pub fn advance(&mut self) -> Result<(), i32> {
        self.revision = self.revision.checked_add(1).ok_or(RESOURCE_LIMIT)?;
        Ok(())
    }
    pub fn clear(&mut self) {
        self.installed.clear();
        self.watermarks.clear();
        self.fingerprints.zeroize();
        let _ = self.advance();
    }
    pub fn validate_install(&self, key: &KeyInstall, peer: [u8; 6]) -> Result<(), i32> {
        if key.reserved != [0; 2]
            || key.version == 0
            || key.rx_sequence[6..] != [0; 2]
            || key.key_len != 16
            || key.key[16..].iter().any(|b| *b != 0)
        {
            return Err(INVALID);
        }
        if key.cipher != CIPHER_CCMP128 {
            return Err(NOT_SUPPORTED);
        }
        if !((key.kind == KEY_PAIRWISE && key.index == 0 && key.peer == peer)
            || (key.kind == KEY_GROUP && key.index <= 3 && key.peer == [0xff; 6]))
        {
            return Err(INVALID);
        }
        let slot = (key.kind, key.index);
        if key.version <= *self.watermarks.get(&slot).unwrap_or(&0) {
            return Err(STATE_CHANGED);
        }
        let fingerprint: [u8; 32] = Sha256::digest(&key.key[..16]).into();
        // Never reinstall the same temporal key within a connection, including
        // after deletion. Prevent accidental PN/replay counter rollback.
        if self.fingerprints.contains(&fingerprint) {
            return Err(STATE_CHANGED);
        }
        if self.fingerprints.len() == 64 {
            return Err(RESOURCE_LIMIT);
        }
        Ok(())
    }
    pub fn prepare_install(&mut self, key: &KeyInstall, peer: [u8; 6]) -> Result<KeyMetadata, i32> {
        self.validate_install(key, peer)?;
        let slot = (key.kind, key.index);
        let fingerprint: [u8; 32] = Sha256::digest(&key.key[..16]).into();
        self.advance()?;
        self.watermarks.insert(slot, key.version);
        self.fingerprints.push(fingerprint);
        Ok(KeyMetadata {
            reference: KeyRef {
                version: key.version,
                kind: key.kind,
                index: key.index,
            },
        })
    }
    pub fn prepare_delete(&mut self, key: KeyRef) -> Result<(), i32> {
        if !self
            .installed
            .get(&(key.kind, key.index))
            .is_some_and(|k| k.reference == key)
        {
            return Err(NOT_FOUND);
        }
        self.advance()
    }
    pub fn installed(&mut self, key: KeyMetadata) {
        self.installed
            .insert((key.reference.kind, key.reference.index), key);
    }
    pub fn replaced_group(&mut self, key: KeyMetadata) {
        self.installed.retain(|(kind, _), _| *kind != KEY_GROUP);
        self.installed(key);
    }
    pub fn deleted(&mut self, key: KeyRef) {
        self.installed.remove(&(key.kind, key.index));
    }
    pub fn flags(&self) -> u32 {
        (if self.installed.keys().any(|(k, _)| *k == KEY_PAIRWISE) {
            SECURITY_PAIRWISE
        } else {
            0
        }) | (if self.installed.keys().any(|(k, _)| *k == KEY_GROUP) {
            SECURITY_GROUP
        } else {
            0
        })
    }
    pub fn ready(&self) -> bool {
        self.flags() == SECURITY_PAIRWISE | SECURITY_GROUP
    }
}
