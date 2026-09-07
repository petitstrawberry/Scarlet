#![no_std]

fn register_prototype() {
    scarlet::println!("[scarlet-module-prototype] Hello from external module!");
}

scarlet::driver_initcall!(register_prototype);

#[used]
static SCARLET_MODULE_PROTOTYPE_ANCHOR: fn() = force_link;

#[inline(never)]
pub fn force_link() {}
