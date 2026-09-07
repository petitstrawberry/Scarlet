//! Guest register indices (guest's perspective)
//!
//! These indices represent what the guest sees, not the underlying CSR names.

pub mod reg {
    // General-purpose registers (x0-x31)
    pub const X0: usize = 0;
    pub const X1: usize = 1;
    pub const X2: usize = 2;
    pub const X3: usize = 3;
    pub const X4: usize = 4;
    pub const X5: usize = 5;
    pub const X6: usize = 6;
    pub const X7: usize = 7;
    pub const X8: usize = 8;
    pub const X9: usize = 9;
    pub const X10: usize = 10;
    pub const X11: usize = 11;
    pub const X12: usize = 12;
    pub const X13: usize = 13;
    pub const X14: usize = 14;
    pub const X15: usize = 15;
    pub const X16: usize = 16;
    pub const X17: usize = 17;
    pub const X18: usize = 18;
    pub const X19: usize = 19;
    pub const X20: usize = 20;
    pub const X21: usize = 21;
    pub const X22: usize = 22;
    pub const X23: usize = 23;
    pub const X24: usize = 24;
    pub const X25: usize = 25;
    pub const X26: usize = 26;
    pub const X27: usize = 27;
    pub const X28: usize = 28;
    pub const X29: usize = 29;
    pub const X30: usize = 30;
    pub const X31: usize = 31;

    // GPR aliases
    pub const ZERO: usize = X0;
    pub const RA: usize = X1;
    pub const SP: usize = X2;
    pub const GP: usize = X3;
    pub const TP: usize = X4;
    pub const T0: usize = X5;
    pub const T1: usize = X6;
    pub const T2: usize = X7;
    pub const S0: usize = X8;
    pub const FP: usize = X8;
    pub const S1: usize = X9;
    pub const A0: usize = X10;
    pub const A1: usize = X11;
    pub const A2: usize = X12;
    pub const A3: usize = X13;
    pub const A4: usize = X14;
    pub const A5: usize = X15;
    pub const A6: usize = X16;
    pub const A7: usize = X17;
    pub const S2: usize = X18;
    pub const S3: usize = X19;
    pub const S4: usize = X20;
    pub const S5: usize = X21;
    pub const S6: usize = X22;
    pub const S7: usize = X23;
    pub const S8: usize = X24;
    pub const S9: usize = X25;
    pub const S10: usize = X26;
    pub const S11: usize = X27;
    pub const T3: usize = X28;
    pub const T4: usize = X29;
    pub const T5: usize = X30;
    pub const T6: usize = X31;

    // Program counter
    pub const PC: usize = 32;

    // System registers (guest's view -> VS-mode CSRs)
    pub const SSTATUS: usize = 33;
    pub const SEPC: usize = 34;
    pub const SCAUSE: usize = 35;
    pub const STVAL: usize = 36;
    pub const STVEC: usize = 37;
    pub const SATP: usize = 38;
    pub const SSCRATCH: usize = 39;
    pub const SIE: usize = 40;
    pub const SIP: usize = 41;

    // FPU registers (f0-f31)
    pub const F0: usize = 64;
    pub const F1: usize = 65;
    pub const F2: usize = 66;
    pub const F3: usize = 67;
    pub const F4: usize = 68;
    pub const F5: usize = 69;
    pub const F6: usize = 70;
    pub const F7: usize = 71;
    pub const F8: usize = 72;
    pub const F9: usize = 73;
    pub const F10: usize = 74;
    pub const F11: usize = 75;
    pub const F12: usize = 76;
    pub const F13: usize = 77;
    pub const F14: usize = 78;
    pub const F15: usize = 79;
    pub const F16: usize = 80;
    pub const F17: usize = 81;
    pub const F18: usize = 82;
    pub const F19: usize = 83;
    pub const F20: usize = 84;
    pub const F21: usize = 85;
    pub const F22: usize = 86;
    pub const F23: usize = 87;
    pub const F24: usize = 88;
    pub const F25: usize = 89;
    pub const F26: usize = 90;
    pub const F27: usize = 91;
    pub const F28: usize = 92;
    pub const F29: usize = 93;
    pub const F30: usize = 94;
    pub const F31: usize = 95;

    // FPU control register
    pub const FCSR: usize = 96;

    pub const FREG_BASE: usize = 64;
    pub const FREG_COUNT: usize = 32;
    pub const IS_FREG: fn(usize) -> bool = |i| i >= F0 && i <= F31;
}
