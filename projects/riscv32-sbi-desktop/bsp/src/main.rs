#![no_std]
#![no_main]

// The kernel provides the physical SBI entry and secondary-hart bootstrap.
use scarlet_modules::force_link;
scarlet_modules::scarlet::early_initcall!(force_link);

// Reserved for cargo-scarlet's post-link kernel symbol table.
#[unsafe(link_section = ".scarlet_ksyms")]
#[used]
static _KSYM_PLACEHOLDER: [u64; 65536] = [0; 65536];
