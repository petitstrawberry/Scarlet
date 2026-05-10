//! Guest register indices (guest's perspective)
//!
//! These indices represent what the guest sees, not the underlying CSR names.

pub mod reg {
    // General-purpose registers (x0-x31)
    pub const X0: u32 = 0;
    pub const X1: u32 = 1;
    pub const X2: u32 = 2;
    pub const X3: u32 = 3;
    pub const X4: u32 = 4;
    pub const X5: u32 = 5;
    pub const X6: u32 = 6;
    pub const X7: u32 = 7;
    pub const X8: u32 = 8;
    pub const X9: u32 = 9;
    pub const X10: u32 = 10;
    pub const X11: u32 = 11;
    pub const X12: u32 = 12;
    pub const X13: u32 = 13;
    pub const X14: u32 = 14;
    pub const X15: u32 = 15;
    pub const X16: u32 = 16;
    pub const X17: u32 = 17;
    pub const X18: u32 = 18;
    pub const X19: u32 = 19;
    pub const X20: u32 = 20;
    pub const X21: u32 = 21;
    pub const X22: u32 = 22;
    pub const X23: u32 = 23;
    pub const X24: u32 = 24;
    pub const X25: u32 = 25;
    pub const X26: u32 = 26;
    pub const X27: u32 = 27;
    pub const X28: u32 = 28;
    pub const X29: u32 = 29;
    pub const X30: u32 = 30;
    pub const X31: u32 = 31;

    // GPR aliases
    pub const ZERO: u32 = X0;
    pub const RA: u32 = X1;
    pub const SP: u32 = X2;
    pub const GP: u32 = X3;
    pub const TP: u32 = X4;
    pub const T0: u32 = X5;
    pub const T1: u32 = X6;
    pub const T2: u32 = X7;
    pub const S0: u32 = X8;
    pub const FP: u32 = X8;
    pub const S1: u32 = X9;
    pub const A0: u32 = X10;
    pub const A1: u32 = X11;
    pub const A2: u32 = X12;
    pub const A3: u32 = X13;
    pub const A4: u32 = X14;
    pub const A5: u32 = X15;
    pub const A6: u32 = X16;
    pub const A7: u32 = X17;
    pub const S2: u32 = X18;
    pub const S3: u32 = X19;
    pub const S4: u32 = X20;
    pub const S5: u32 = X21;
    pub const S6: u32 = X22;
    pub const S7: u32 = X23;
    pub const S8: u32 = X24;
    pub const S9: u32 = X25;
    pub const S10: u32 = X26;
    pub const S11: u32 = X27;
    pub const T3: u32 = X28;
    pub const T4: u32 = X29;
    pub const T5: u32 = X30;
    pub const T6: u32 = X31;

    // Program counter
    pub const PC: u32 = 32;

    // System registers (guest's view -> VS-mode CSRs)
    pub const SSTATUS: u32 = 33;
    pub const SEPC: u32 = 34;
    pub const SCAUSE: u32 = 35;
    pub const STVAL: u32 = 36;
    pub const STVEC: u32 = 37;
    pub const SATP: u32 = 38;
    pub const SSCRATCH: u32 = 39;
    pub const SIE: u32 = 40;
    pub const SIP: u32 = 41;

    // FPU registers (f0-f31)
    pub const F0: u32 = 64;
    pub const F1: u32 = 65;
    pub const F2: u32 = 66;
    pub const F3: u32 = 67;
    pub const F4: u32 = 68;
    pub const F5: u32 = 69;
    pub const F6: u32 = 70;
    pub const F7: u32 = 71;
    pub const F8: u32 = 72;
    pub const F9: u32 = 73;
    pub const F10: u32 = 74;
    pub const F11: u32 = 75;
    pub const F12: u32 = 76;
    pub const F13: u32 = 77;
    pub const F14: u32 = 78;
    pub const F15: u32 = 79;
    pub const F16: u32 = 80;
    pub const F17: u32 = 81;
    pub const F18: u32 = 82;
    pub const F19: u32 = 83;
    pub const F20: u32 = 84;
    pub const F21: u32 = 85;
    pub const F22: u32 = 86;
    pub const F23: u32 = 87;
    pub const F24: u32 = 88;
    pub const F25: u32 = 89;
    pub const F26: u32 = 90;
    pub const F27: u32 = 91;
    pub const F28: u32 = 92;
    pub const F29: u32 = 93;
    pub const F30: u32 = 94;
    pub const F31: u32 = 95;

    // FPU control register
    pub const FCSR: u32 = 96;

    pub const FREG_BASE: u32 = 64;
    pub const FREG_COUNT: u32 = 32;
    pub const IS_FREG: fn(u32) -> bool = |i| i >= F0 && i <= F31;
}
