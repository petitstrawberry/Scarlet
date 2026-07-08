/// Device-visible buffer range.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AvdDmaRange {
    /// Device-visible DMA address.
    pub dma_addr: u64,
    /// Byte length of the range.
    pub len: usize,
}
