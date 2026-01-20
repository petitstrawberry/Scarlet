use bitflags::bitflags;

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    pub struct DirtyFlags: u8 {
        const LAYOUT = 1;
        const PAINT = 2;
        const CHILDREN = 4;
    }
}
