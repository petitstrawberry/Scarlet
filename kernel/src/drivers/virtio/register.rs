#[derive(Debug, Clone, Copy)]
pub enum Register {
    MagicValue = 0x00,
    Version = 0x04,
    DeviceId = 0x08,
    VendorId = 0x0c,
    DeviceFeatures = 0x10,
    DriverFeatures = 0x20,
    QueueSel = 0x30,
    QueueNumMax = 0x34,
    QueueNum = 0x38,
    QueueAlign = 0x3c,
    QueuePfn = 0x40,
    QueueReady = 0x44,
    QueueNotify = 0x50,
    InterruptStatus = 0x60,
    InterruptAck = 0x64,
    Status = 0x70,
    QueueDescLow = 0x80,
    QueueDescHigh = 0x84,
    DriverDescLow = 0x90,
    DriverDescHigh = 0x94,
    DeviceDescLow = 0xa0,
    DeviceDescHigh = 0xa4,
    DeviceConfig = 0x100,
}

impl Register {
    pub fn offset(&self) -> usize {
        *self as usize
    }
}

