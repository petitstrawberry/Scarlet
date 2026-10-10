//! An exclusively owned transmit buffer with initialized header space.
//!
//! Layers consume the buffer rather than borrowing temporary serialized packets.
//! No raw pointers or shared mutable backing escape this type. Device queues take
//! ownership of the final Vec; DMA ownership remains the driver's responsibility.

use super::socket::SocketError;
use alloc::vec::Vec;

/// Header space for IPv4 over Ethernet; transport headers belong to the payload.
pub(crate) const IPV4_ETHERNET_HEADROOM: usize = 20 + 14;

#[derive(Debug)]
pub struct PacketBuffer {
    storage: Vec<u8>,
    start: usize,
}

impl PacketBuffer {
    /// Reserve initialized prefix bytes and a payload, with capacity for Ethernet
    /// minimum-frame padding. Unused prefix bytes are never exposed to a layer.
    pub fn new(payload_len: usize, headroom: usize) -> Result<Self, SocketError> {
        let total = payload_len
            .checked_add(headroom)
            .ok_or(SocketError::InvalidPacket)?;
        let mut storage = Vec::new();
        storage
            .try_reserve_exact(total.max(60))
            .map_err(|_| SocketError::Other("packet allocation failed".into()))?;
        storage.resize(total, 0);
        Ok(Self {
            storage,
            start: headroom,
        })
    }

    pub fn from_slice(payload: &[u8], headroom: usize) -> Result<Self, SocketError> {
        let total = payload
            .len()
            .checked_add(headroom)
            .ok_or(SocketError::InvalidPacket)?;
        let mut storage = Vec::new();
        storage
            .try_reserve_exact(total.max(60))
            .map_err(|_| SocketError::Other("packet allocation failed".into()))?;
        // Initialize only the prefix before copying the payload. Resizing the
        // entire buffer first would write every payload byte twice.
        storage.resize(headroom, 0);
        storage.extend_from_slice(payload);
        Ok(Self {
            storage,
            start: headroom,
        })
    }

    /// Adopt existing owned bytes. Headers can still use the checked fallback.
    pub fn from_vec(storage: Vec<u8>) -> Self {
        Self { storage, start: 0 }
    }

    pub fn len(&self) -> usize {
        self.storage.len() - self.start
    }
    pub fn as_slice(&self) -> &[u8] {
        &self.storage[self.start..]
    }
    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        &mut self.storage[self.start..]
    }

    /// Prepend without moving the payload when sufficient headroom was reserved.
    /// Arbitrary callers with insufficient headroom use a checked owned fallback.
    pub fn prepend(&mut self, header: &[u8]) -> Result<(), SocketError> {
        if header.len() > self.start {
            *self = Self::from_slice(self.as_slice(), header.len())?;
        }
        self.start -= header.len();
        self.storage[self.start..self.start + header.len()].copy_from_slice(header);
        Ok(())
    }

    /// Padding is initialized and becomes part of the transmitted slice.
    pub fn pad_to(&mut self, minimum: usize) -> Result<(), SocketError> {
        if minimum > self.len() {
            let total = self
                .start
                .checked_add(minimum)
                .ok_or(SocketError::InvalidPacket)?;
            self.storage
                .try_reserve_exact(total - self.storage.len())
                .map_err(|_| SocketError::Other("packet allocation failed".into()))?;
            self.storage.resize(total, 0);
        }
        Ok(())
    }

    /// Transfer the final allocation to the device. The normal TCP/IP/Ethernet
    /// path has consumed all headroom, so this does not move payload bytes.
    /// A caller finalizing with unused headroom gets an in-place compaction.
    pub fn into_vec(mut self) -> Vec<u8> {
        if self.start != 0 {
            let len = self.len();
            self.storage.copy_within(self.start.., 0);
            self.storage.truncate(len);
        }
        self.storage
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn packet_headers_preserve_payload_address_and_final_allocation() {
        let payload = [0xa5; 1460];
        let mut packet = PacketBuffer::from_slice(&payload, IPV4_ETHERNET_HEADROOM).unwrap();
        let allocation = packet.storage.as_ptr();
        let payload_address = packet.as_slice().as_ptr();
        packet.prepend(&[0x45; 20]).unwrap();
        packet.prepend(&[0xee; 14]).unwrap();
        packet.pad_to(60).unwrap();
        assert_eq!(packet.storage.as_ptr(), allocation);
        assert_eq!(packet.as_slice()[34..].as_ptr(), payload_address);
        let frame = packet.into_vec();
        assert_eq!(frame.as_ptr(), allocation);
        assert_eq!(&frame[34..], &payload);
    }

    #[test_case]
    fn packet_padding_and_arp_compaction_do_not_expose_headroom() {
        let mut packet = PacketBuffer::from_slice(&[7; 20], 34).unwrap();
        packet.prepend(&[0x45; 20]).unwrap();
        let allocation = packet.storage.as_ptr();
        let queued_ip = packet.into_vec();
        assert_eq!(queued_ip.as_ptr(), allocation);
        assert_eq!(queued_ip.len(), 40);
        assert_eq!(&queued_ip[..20], &[0x45; 20]);
        assert_eq!(&queued_ip[20..], &[7; 20]);
        let mut packet = PacketBuffer::from_slice(&queued_ip, 14).unwrap();
        packet.prepend(&[0xee; 14]).unwrap();
        let allocation = packet.storage.as_ptr();
        packet.pad_to(60).unwrap();
        let frame = packet.into_vec();
        assert_eq!(frame.as_ptr(), allocation);
        assert_eq!(&frame[14..54], queued_ip.as_slice());
        assert_eq!(&frame[54..], &[0; 6]);
    }

    #[test_case]
    fn packet_insufficient_headroom_has_safe_owned_fallback() {
        let original = alloc::vec![1, 2, 3];
        let mut packet = PacketBuffer::from_slice(&original, 1).unwrap();
        packet.prepend(&[4, 5, 6]).unwrap();
        drop(original);
        assert_eq!(packet.into_vec(), [4, 5, 6, 1, 2, 3]);
    }

    #[test_case]
    fn packet_length_overflow_is_rejected() {
        assert!(matches!(
            PacketBuffer::new(usize::MAX, 1),
            Err(SocketError::InvalidPacket)
        ));
        let mut packet = PacketBuffer::new(0, 1).unwrap();
        assert_eq!(packet.pad_to(usize::MAX), Err(SocketError::InvalidPacket));
        assert!(packet.as_slice().is_empty());
    }
}
