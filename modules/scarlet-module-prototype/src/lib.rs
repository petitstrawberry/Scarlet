#![no_std]

#[used]
static SCARLET_MODULE_PROTOTYPE_ANCHOR: fn() = force_link;

#[inline(never)]
pub fn force_link() {}
