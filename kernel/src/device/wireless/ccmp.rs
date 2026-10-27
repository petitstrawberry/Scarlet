//! Software CCMP-128 for unfragmented three-address infrastructure STA data.
//! Callers serialize key changes and PN allocation with data admission/retirement.
extern crate alloc;
use super::auth::{Association, KeyState, valid_eapol, valid_peer};
use aes::Aes128;
use alloc::{collections::BTreeMap, vec::Vec};
use ccm::{
    AeadInOut, Ccm, KeyInit,
    consts::{U8, U13},
};
use scarlet_abi::wireless::*;
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

type Cipher = Ccm<Aes128, U8, U13>;
pub const MAX_PN: u64 = (1u64 << 48) - 1;
const HISTORY_LIMIT: usize = 256;
const MAX_ETHERNET: usize = 1514;
const SNAP: [u8; 6] = [0xaa, 0xaa, 3, 0, 0, 0];
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Invalid,
    NoKey,
    Replay,
    Integrity,
    Exhausted,
    Scope,
    KeyReuse,
    Capacity,
    NotControl,
}
// Fingerprints and watermarks survive key retirement, without retaining secrets.
// A repeated GTK can be legitimate on reconnect: retain its per-TID RX floor.
struct History {
    fingerprint: [u8; 32],
    rx: [u64; 16],
    tx: u64,
    pairwise: bool,
}
struct Key {
    value: Zeroizing<[u8; 16]>,
    reference: KeyRef,
    history: usize,
}
pub struct KeyRing {
    scope: Option<Association>,
    ledger: KeyState,
    keys: BTreeMap<(u32, u32), Key>,
    history: Vec<History>,
    stats: CcmpStatus,
}
impl Default for KeyRing {
    fn default() -> Self {
        Self {
            scope: None,
            ledger: KeyState::default(),
            keys: BTreeMap::new(),
            history: Vec::new(),
            stats: CcmpStatus::default(),
        }
    }
}
impl KeyRing {
    pub fn clear(&mut self) {
        self.stats.retired += self.keys.len() as u64;
        self.keys.clear();
        self.ledger.clear();
        self.scope = None;
    }
    pub fn status(&self) -> CcmpStatus {
        CcmpStatus {
            keys: self.keys.len() as u32,
            tx_pn: self
                .keys
                .get(&(KEY_PAIRWISE, 0))
                .map_or(0, |k| self.history[k.history].tx),
            largest_rx_pn: self
                .keys
                .values()
                .flat_map(|k| self.history[k.history].rx)
                .max()
                .unwrap_or(0),
            ..self.stats
        }
    }
    pub fn ready(&self) -> bool {
        self.ledger.ready()
    }
    pub fn install(&mut self, scope: Association, request: &KeyInstall) -> Result<(), Error> {
        if scope.device == 0 || scope.connection == 0 || !valid_peer(scope.peer) {
            return Err(Error::Invalid);
        }
        if self.scope.is_some_and(|s| s != scope) {
            return Err(Error::Scope);
        }
        self.ledger
            .validate_install(request, scope.peer)
            .map_err(|_| Error::KeyReuse)?;
        let fingerprint: [u8; 32] = Sha256::digest(&request.key[..16]).into();
        let found = self
            .history
            .iter()
            .position(|h| h.fingerprint == fingerprint);
        // Fresh pairwise TK is mandatory after association retirement. Never
        // reset a transmitter's nonce sequence by reinstalling an old TK.
        if found.is_some_and(|i| request.kind == KEY_PAIRWISE || self.history[i].pairwise) {
            return Err(Error::KeyReuse);
        }
        if found.is_none() && self.history.len() == HISTORY_LIMIT {
            return Err(Error::Capacity);
        }
        let metadata = self
            .ledger
            .prepare_install(request, scope.peer)
            .map_err(|_| Error::KeyReuse)?;
        let index = found.unwrap_or(self.history.len());
        if found.is_none() {
            self.history.push(History {
                fingerprint,
                rx: [0; 16],
                tx: 0,
                pairwise: request.kind == KEY_PAIRWISE,
            });
        }
        let mut pn = [0; 8];
        pn[..6].copy_from_slice(&request.rx_sequence[..6]);
        let floor = u64::from_le_bytes(pn);
        for watermark in &mut self.history[index].rx {
            *watermark = (*watermark).max(floor);
        }
        let mut value = Zeroizing::new([0; 16]);
        value.copy_from_slice(&request.key[..16]);
        let replaced = self.keys.insert(
            (request.kind, request.index),
            Key {
                value,
                reference: metadata.reference,
                history: index,
            },
        );
        if replaced.is_some() {
            self.stats.retired += 1;
        }
        self.ledger.installed(metadata);
        self.scope = Some(scope);
        Ok(())
    }
    /// Called under the ingress barrier and state lock. A failed install leaves
    /// every old slot intact; success retires all other GTK slots atomically.
    pub fn replace_group(&mut self, scope: Association, request: &KeyInstall) -> Result<(), Error> {
        if request.kind != KEY_GROUP || !self.ready() {
            return Err(Error::Invalid);
        }
        self.install(scope, request)?;
        let old: Vec<_> = self
            .keys
            .keys()
            .copied()
            .filter(|(kind, index)| *kind == KEY_GROUP && *index != request.index)
            .collect();
        for slot in old {
            self.keys.remove(&slot);
            self.stats.retired += 1;
        }
        let metadata = self.ledger.installed[&(KEY_GROUP, request.index)];
        self.ledger.replaced_group(metadata);
        Ok(())
    }
    pub fn delete(&mut self, scope: Association, reference: KeyRef) -> Result<(), Error> {
        if self.scope != Some(scope) {
            return Err(Error::Scope);
        }
        let slot = (reference.kind, reference.index);
        if !self
            .keys
            .get(&slot)
            .is_some_and(|k| k.reference == reference)
        {
            return Err(Error::NoKey);
        }
        self.ledger
            .prepare_delete(reference)
            .map_err(|_| Error::NoKey)?;
        self.keys.remove(&slot);
        self.ledger.deleted(reference);
        self.stats.retired += 1;
        Ok(())
    }
    pub fn seal(
        &mut self,
        scope: Association,
        own: [u8; 6],
        ethernet: &[u8],
        tid: Option<u8>,
    ) -> Result<Vec<u8>, Error> {
        if self.scope != Some(scope) {
            return Err(Error::Scope);
        }
        if !valid_peer(own)
            || own == scope.peer
            || ethernet.len() < 14
            || ethernet.len() > MAX_ETHERNET
            || ethernet[6..12] != own
            || tid.is_some_and(|t| t > 15)
            || u16::from_be_bytes([ethernet[12], ethernet[13]]) < 1536
        {
            return Err(Error::Invalid);
        }
        if ethernet[12..14] == [0x88, 0x8e]
            && (ethernet[..6] != scope.peer || !valid_eapol(&ethernet[14..]))
        {
            return Err(Error::Invalid);
        }
        let key = self.keys.get(&(KEY_PAIRWISE, 0)).ok_or(Error::NoKey)?;
        let history = &mut self.history[key.history];
        if history.tx == MAX_PN {
            return Err(Error::Exhausted);
        }
        // Burn before encryption/submission; failures never roll a PN back.
        history.tx += 1;
        let pn = history.tx;
        let mut header = Vec::with_capacity(26);
        header.extend_from_slice(&[if tid.is_some() { 0x88 } else { 8 }, 0x41, 0, 0]);
        header.extend_from_slice(&scope.peer);
        header.extend_from_slice(&own);
        header.extend_from_slice(&ethernet[..6]);
        header.extend_from_slice(&[0, 0]);
        if let Some(tid) = tid {
            header.extend_from_slice(&[tid, 0]);
        }
        let (aad, nonce) = aad_nonce(&header, pn, tid);
        let mut payload = Zeroizing::new(Vec::with_capacity(ethernet.len() - 6));
        payload.extend_from_slice(&SNAP);
        payload.extend_from_slice(&ethernet[12..]);
        let cipher = Cipher::new((&*key.value).into());
        let tag = cipher
            .encrypt_inout_detached(&nonce.into(), &aad, payload.as_mut_slice().into())
            .map_err(|_| Error::Integrity)?;
        let bytes = pn.to_le_bytes();
        header.extend_from_slice(&[
            bytes[0], bytes[1], 0, 0x20, bytes[2], bytes[3], bytes[4], bytes[5],
        ]);
        header.extend_from_slice(&payload);
        header.extend_from_slice(&tag);
        Ok(header)
    }
    pub fn open(
        &mut self,
        scope: Association,
        own: [u8; 6],
        mpdu: &[u8],
    ) -> Result<Vec<u8>, Error> {
        let result = self.open_inner(scope, own, mpdu, false);
        match result {
            Ok(_) => self.stats.rx_accepted += 1,
            Err(Error::Replay) => self.stats.replay_dropped += 1,
            Err(Error::Integrity) => self.stats.integrity_dropped += 1,
            Err(_) => self.stats.invalid_dropped += 1,
        };
        result
    }
    /// Classify a held ciphertext without advancing ordinary-data RX state.
    /// Only authenticated EAPOL consumes PN; ordinary data stays ciphertext.
    pub fn open_control(
        &mut self,
        scope: Association,
        own: [u8; 6],
        mpdu: &[u8],
    ) -> Result<Vec<u8>, Error> {
        let result = self.open_inner(scope, own, mpdu, true);
        match result {
            Ok(_) => self.stats.rx_accepted += 1,
            Err(Error::NotControl) => {}
            Err(Error::Replay) => self.stats.replay_dropped += 1,
            Err(Error::Integrity) => self.stats.integrity_dropped += 1,
            Err(_) => self.stats.invalid_dropped += 1,
        };
        result
    }
    fn open_inner(
        &mut self,
        scope: Association,
        own: [u8; 6],
        mpdu: &[u8],
        control_only: bool,
    ) -> Result<Vec<u8>, Error> {
        if self.scope != Some(scope) {
            return Err(Error::Scope);
        }
        if mpdu.len() < 48
            || mpdu.len() > MAX_ETHERNET + 50
            || !matches!(mpdu[0], 8 | 0x88)
            || mpdu[1] & 0xc7 != 0x42
            || mpdu[22] & 15 != 0
        {
            return Err(Error::Invalid);
        }
        let tid = if mpdu[0] == 0x88 {
            if mpdu[24] & 0x80 != 0 || mpdu[25] != 0 {
                return Err(Error::Invalid);
            }
            Some(mpdu[24] & 15)
        } else {
            None
        };
        let len = if tid.is_some() { 26 } else { 24 };
        if mpdu.len() < len + 24
            || mpdu[10..16] != scope.peer
            || (mpdu[4] & 1 == 0 && mpdu[4..10] != own)
        {
            return Err(Error::Invalid);
        }
        let iv = &mpdu[len..len + 8];
        if iv[2] != 0 || iv[3] & 0x3f != 0x20 {
            return Err(Error::Invalid);
        }
        let kind = if mpdu[4] & 1 != 0 {
            KEY_GROUP
        } else {
            KEY_PAIRWISE
        };
        let index = u32::from(iv[3] >> 6);
        if kind == KEY_PAIRWISE && index != 0 {
            return Err(Error::Invalid);
        }
        let key = self.keys.get(&(kind, index)).ok_or(Error::NoKey)?;
        let pn = u64::from_le_bytes([iv[0], iv[1], iv[4], iv[5], iv[6], iv[7], 0, 0]);
        // Non-QoS shares priority 0 with QoS TID 0 (same nonce domain).
        let queue = usize::from(tid.unwrap_or(0));
        let history = &mut self.history[key.history];
        if pn <= history.rx[queue] {
            return Err(Error::Replay);
        }
        let (aad, nonce) = aad_nonce(&mpdu[..len], pn, tid);
        let mut payload = Zeroizing::new(mpdu[len + 8..mpdu.len() - 8].to_vec());
        let cipher = Cipher::new((&*key.value).into());
        let tag: [u8; 8] = mpdu[mpdu.len() - 8..]
            .try_into()
            .map_err(|_| Error::Invalid)?;
        cipher
            .decrypt_inout_detached(
                &nonce.into(),
                &aad,
                payload.as_mut_slice().into(),
                &tag.into(),
            )
            .map_err(|_| Error::Integrity)?;
        if payload.len() < 8
            || payload[..6] != SNAP
            || u16::from_be_bytes([payload[6], payload[7]]) < 1536
        {
            return Err(Error::Invalid);
        }
        let eapol = payload[6..8] == [0x88, 0x8e];
        if eapol
            && (kind != KEY_PAIRWISE
                || mpdu[16..22] != scope.peer
                || mpdu[4..10] != own
                || !valid_eapol(&payload[8..]))
        {
            return Err(Error::Invalid);
        }
        if control_only && !eapol {
            return Err(Error::NotControl);
        }
        let mut ethernet = Vec::with_capacity(6 + payload.len());
        ethernet.extend_from_slice(&mpdu[4..10]);
        ethernet.extend_from_slice(&mpdu[16..22]);
        ethernet.extend_from_slice(&payload[6..]);
        if ethernet.len() > MAX_ETHERNET {
            return Err(Error::Invalid);
        }
        // Commit only after MIC and supported payload validation; serialized
        // with other RX and key teardown by the owning facade's state lock.
        history.rx[queue] = pn;
        Ok(ethernet)
    }
}
fn aad_nonce(header: &[u8], pn: u64, tid: Option<u8>) -> (Vec<u8>, [u8; 13]) {
    let mut aad = Vec::with_capacity(24);
    aad.extend_from_slice(&[header[0] & 0x8f, header[1] & 0xc7]);
    aad.extend_from_slice(&header[4..22]);
    aad.extend_from_slice(&[header[22] & 15, 0]);
    if let Some(t) = tid {
        aad.extend_from_slice(&[t, 0]);
    }
    let mut nonce = [0; 13];
    nonce[0] = tid.unwrap_or(0);
    nonce[1..7].copy_from_slice(&header[10..16]);
    nonce[7..].copy_from_slice(&pn.to_be_bytes()[2..]);
    (aad, nonce)
}

#[cfg(test)]
mod tests {
    use super::*;
    const OWN: [u8; 6] = [0x52, 0x54, 0, 0x12, 0x34, 0];
    fn scope(n: u64) -> Association {
        Association {
            device: 1,
            connection: n,
            peer: [2, 0, 0, 0, 0, 1],
        }
    }
    fn key(kind: u32) -> KeyInstall {
        let mut k = KeyInstall {
            version: 1,
            kind,
            index: if kind == KEY_GROUP { 1 } else { 0 },
            cipher: CIPHER_CCMP128,
            key_len: 16,
            peer: if kind == KEY_GROUP {
                [255; 6]
            } else {
                scope(1).peer
            },
            ..KeyInstall::default()
        };
        for (i, b) in k.key[..16].iter_mut().enumerate() {
            *b = i as u8 + if kind == KEY_GROUP { 16 } else { 0 };
        }
        k
    }
    fn setup() -> KeyRing {
        let mut r = KeyRing::default();
        r.install(scope(1), &key(KEY_PAIRWISE)).unwrap();
        r.install(scope(1), &key(KEY_GROUP)).unwrap();
        r
    }
    fn hex(s: &str) -> Vec<u8> {
        s.as_bytes()
            .chunks_exact(2)
            .map(|c| u8::from_str_radix(core::str::from_utf8(c).unwrap(), 16).unwrap())
            .collect()
    }
    fn vector(name: &str) -> (Vec<u8>, Vec<u8>) {
        let line = include_str!("../../../../tools/wireless-ccmp/vectors.txt")
            .lines()
            .find(|l| l.starts_with(&alloc::format!("{name} ")))
            .unwrap();
        let v: Vec<_> = line.split_whitespace().collect();
        (hex(v[1]), hex(v[2]))
    }
    #[cfg_attr(target_os = "none", test_case)]
    #[cfg_attr(not(target_os = "none"), test)]
    fn rfc3610_packet_vector_one() {
        let key = hex("c0c1c2c3c4c5c6c7c8c9cacbcccdcecf");
        let nonce: [u8; 13] = hex("00000003020100a0a1a2a3a4a5").try_into().unwrap();
        let aad: Vec<u8> = (0..8).collect();
        let mut data: Vec<u8> = (8..31).collect();
        let cipher = Cipher::new(key.as_slice().try_into().unwrap());
        let tag = cipher
            .encrypt_inout_detached(&nonce.into(), &aad, data.as_mut_slice().into())
            .unwrap();
        data.extend_from_slice(&tag);
        assert_eq!(
            data,
            hex("588c979a61c663d2f066d0c2c0f989806d5f6b61dac38417e8d12cfdf926e0")
        );
    }
    #[cfg_attr(target_os = "none", test_case)]
    #[cfg_attr(not(target_os = "none"), test)]
    fn independent_ccmp_vectors_unicast_group_and_qos() {
        for name in ["rx", "rxq0", "rxq7", "group"] {
            let mut r = setup();
            let (eth, mpdu) = vector(name);
            assert_eq!(r.open(scope(1), OWN, &mpdu), Ok(eth));
        }
        for (name, tid) in [("tx", None), ("txq", Some(7))] {
            let mut r = setup();
            let (eth, mpdu) = vector(name);
            assert_eq!(r.seal(scope(1), OWN, &eth, tid), Ok(mpdu));
        }
    }
    #[cfg_attr(target_os = "none", test_case)]
    #[cfg_attr(not(target_os = "none"), test)]
    fn replay_is_per_tid_and_non_qos_shares_priority_zero() {
        let mut shared = setup();
        let (_, q0) = vector("rxq0");
        let (_, nq) = vector("rx");
        shared.open(scope(1), OWN, &q0).unwrap();
        assert_eq!(shared.open(scope(1), OWN, &nq), Err(Error::Replay));
        let mut r = setup();
        for name in ["rx", "rxq0", "rxq7", "group"] {
            let (_, p) = vector(name);
            assert!(r.open(scope(1), OWN, &p).is_ok());
            assert_eq!(r.open(scope(1), OWN, &p), Err(Error::Replay));
        }
        assert_eq!(r.status().replay_dropped, 4);
        assert_eq!(r.status().rx_accepted, 4);
    }
    #[cfg_attr(target_os = "none", test_case)]
    #[cfg_attr(not(target_os = "none"), test)]
    fn bad_mic_does_not_poison_high_watermark_or_release_plaintext() {
        let mut r = setup();
        let (eth, p) = vector("rx");
        let mut bad = p.clone();
        bad[31] = 255; // spoof a much higher PN with invalid MIC
        assert_eq!(r.open(scope(1), OWN, &bad), Err(Error::Integrity));
        assert_eq!(r.status().largest_rx_pn, 0);
        assert_eq!(r.open(scope(1), OWN, &p), Ok(eth));
        assert_eq!(r.status().integrity_dropped, 1);
    }
    #[cfg_attr(target_os = "none", test_case)]
    #[cfg_attr(not(target_os = "none"), test)]
    fn authenticated_addresses_and_restricted_header_forms() {
        let (_, p) = vector("rx");
        for (at, mask) in [(4, 2), (10, 2), (16, 2), (1, 4), (22, 1), (26, 1), (27, 1)] {
            let mut r = setup();
            let mut bad = p.clone();
            bad[at] ^= mask;
            assert!(r.open(scope(1), OWN, &bad).is_err());
            assert_eq!(r.status().rx_accepted, 0);
        }
        let (_, q) = vector("rxq7");
        let mut bad = q.clone();
        bad[24] |= 0x80;
        assert_eq!(setup().open(scope(1), OWN, &bad), Err(Error::Invalid));
    }
    #[cfg_attr(target_os = "none", test_case)]
    #[cfg_attr(not(target_os = "none"), test)]
    fn reinstall_delete_and_retired_pairwise_material_cannot_reset_pn() {
        let mut r = setup();
        let (eth, _) = vector("tx");
        r.seal(scope(1), OWN, &eth, None).unwrap();
        let mut k = key(KEY_PAIRWISE);
        k.version = 2;
        assert_eq!(r.install(scope(1), &k), Err(Error::KeyReuse));
        assert_eq!(r.status().tx_pn, 1);
        r.delete(
            scope(1),
            KeyRef {
                version: 1,
                kind: KEY_PAIRWISE,
                index: 0,
            },
        )
        .unwrap();
        assert_eq!(r.install(scope(1), &k), Err(Error::KeyReuse));
        r.clear();
        k.version = 1;
        assert_eq!(r.install(scope(2), &k), Err(Error::KeyReuse));
    }
    #[cfg_attr(target_os = "none", test_case)]
    #[cfg_attr(not(target_os = "none"), test)]
    fn gtk_reuse_after_reconnect_preserves_replay_floor() {
        let mut r = setup();
        let (_, p) = vector("group");
        r.open(scope(1), OWN, &p).unwrap();
        r.clear();
        assert_eq!(r.status().keys, 0);
        r.install(scope(2), &key(KEY_GROUP)).unwrap();
        assert_eq!(r.open(scope(2), OWN, &p), Err(Error::Replay));
        assert_eq!(r.status().largest_rx_pn, 3);
        let mut r = KeyRing::default();
        let mut gtk = key(KEY_GROUP);
        gtk.rx_sequence[0] = 3;
        r.install(scope(1), &gtk).unwrap();
        assert_eq!(r.open(scope(1), OWN, &p), Err(Error::Replay));
    }
    #[cfg_attr(target_os = "none", test_case)]
    #[cfg_attr(not(target_os = "none"), test)]
    fn old_scope_and_old_ciphertext_cannot_cross_fresh_key() {
        let mut r = setup();
        let (_, p) = vector("rx");
        r.clear();
        assert_eq!(r.open(scope(1), OWN, &p), Err(Error::Scope));
        let mut new = key(KEY_PAIRWISE);
        new.key[0] = 42;
        r.install(scope(2), &new).unwrap();
        assert_eq!(r.open(scope(1), OWN, &p), Err(Error::Scope));
        assert_eq!(r.open(scope(2), OWN, &p), Err(Error::Integrity));
        assert_eq!(r.status().largest_rx_pn, 0);
    }
    #[cfg_attr(target_os = "none", test_case)]
    #[cfg_attr(not(target_os = "none"), test)]
    fn failed_submission_burns_pn_and_exhaustion_never_wraps() {
        let mut r = setup();
        let (eth, _) = vector("tx");
        let _not_submitted = r.seal(scope(1), OWN, &eth, None).unwrap();
        r.seal(scope(1), OWN, &eth, None).unwrap();
        assert_eq!(r.status().tx_pn, 2);
        let i = r.keys[&(KEY_PAIRWISE, 0)].history;
        r.history[i].tx = MAX_PN - 1;
        let last = r.seal(scope(1), OWN, &eth, None).unwrap();
        assert_eq!(&last[24..32], &[255, 255, 0, 32, 255, 255, 255, 255]);
        assert_eq!(r.seal(scope(1), OWN, &eth, None), Err(Error::Exhausted));
        assert_eq!(r.status().tx_pn, MAX_PN);
    }
    #[cfg_attr(target_os = "none", test_case)]
    #[cfg_attr(not(target_os = "none"), test)]
    fn bounded_history_fails_closed_instead_of_forgetting_old_keys() {
        let mut r = KeyRing::default();
        for n in 0..HISTORY_LIMIT {
            let mut k = key(KEY_GROUP);
            k.key[..2].copy_from_slice(&(n as u16).to_le_bytes());
            r.install(scope(n as u64 + 1), &k).unwrap();
            r.clear();
        }
        let mut k = key(KEY_GROUP);
        k.key[..2].copy_from_slice(&(HISTORY_LIMIT as u16).to_le_bytes());
        assert_eq!(r.install(scope(999), &k), Err(Error::Capacity));
        assert_eq!(r.status().keys, 0);
    }
    #[cfg_attr(target_os = "none", test_case)]
    #[cfg_attr(not(target_os = "none"), test)]
    fn protected_eapol_is_unicast_ptk_only_and_control_preview_does_not_consume_data() {
        let mut r = setup();
        let (eth, p) = vector("eapol");
        assert_eq!(r.open_control(scope(1), OWN, &p), Ok(eth));
        assert_eq!(r.open(scope(1), OWN, &p), Err(Error::Replay));
        let (_, p) = vector("group-eapol");
        assert_eq!(r.open_control(scope(1), OWN, &p), Err(Error::Invalid));
        let mut r = setup();
        let (eth, p) = vector("rx");
        assert_eq!(r.open_control(scope(1), OWN, &p), Err(Error::NotControl));
        assert_eq!(r.status().rx_accepted, 0);
        assert_eq!(r.status().largest_rx_pn, 0);
        assert_eq!(r.open(scope(1), OWN, &p), Ok(eth));
        let outbound = scope(1)
            .peer
            .into_iter()
            .chain(OWN)
            .chain([0x88, 0x8e, 2, 3, 0, 4, 1, 2, 3, 4])
            .collect::<Vec<_>>();
        assert!(r.seal(scope(1), OWN, &outbound, None).is_ok());
        assert_eq!(r.status().tx_pn, 1);
    }
    #[cfg_attr(target_os = "none", test_case)]
    #[cfg_attr(not(target_os = "none"), test)]
    fn atomic_group_cutover_retires_slots_and_preserves_ptk_and_replay_floors() {
        let mut r = setup();
        let (tx, _) = vector("tx");
        r.seal(scope(1), OWN, &tx, None).unwrap();
        let (_, old) = vector("group");
        r.open(scope(1), OWN, &old).unwrap();
        let mut next = key(KEY_GROUP);
        next.index = 2;
        next.key[0] = 42;
        next.rx_sequence[0] = 10;
        r.replace_group(scope(1), &next).unwrap();
        assert!(r.ready());
        assert_eq!(r.status().keys, 2);
        assert_eq!(r.status().tx_pn, 1);
        assert_eq!(r.open(scope(1), OWN, &old), Err(Error::NoKey));
        let (eth, new) = vector("rekey-group");
        assert_eq!(r.open(scope(1), OWN, &new), Ok(eth));
        next.version = 2;
        next.rx_sequence[0] = 0;
        assert_eq!(r.replace_group(scope(1), &next), Err(Error::KeyReuse));
        assert_eq!(r.open(scope(1), OWN, &new), Err(Error::Replay));
        r.seal(scope(1), OWN, &tx, None).unwrap();
        assert_eq!(r.status().tx_pn, 2);
        next.key[0] = 43;
        next.rx_sequence[0] = 20;
        r.replace_group(scope(1), &next).unwrap();
        let (_, old_future) = vector("rekey-old-future");
        assert_eq!(r.open(scope(1), OWN, &old_future), Err(Error::Integrity));
        let (eth, new) = vector("rekey-group2");
        assert_eq!(r.open(scope(1), OWN, &new), Ok(eth));
        assert_eq!(r.status().keys, 2);
        assert_eq!(r.status().tx_pn, 2);
    }
    #[cfg_attr(target_os = "none", test_case)]
    #[cfg_attr(not(target_os = "none"), test)]
    fn failed_group_candidate_leaves_old_data_and_pn_intact() {
        let mut r = setup();
        let (tx, _) = vector("tx");
        r.seal(scope(1), OWN, &tx, None).unwrap();
        let mut bad = key(KEY_GROUP);
        bad.index = 2;
        bad.key_len = 17;
        bad.key[0] = 42;
        assert!(r.replace_group(scope(1), &bad).is_err());
        assert!(r.ready());
        assert_eq!(
            (r.status().keys, r.status().tx_pn, r.status().retired),
            (2, 1, 0)
        );
        let (eth, old) = vector("group");
        assert_eq!(r.open(scope(1), OWN, &old), Ok(eth));
    }
}
