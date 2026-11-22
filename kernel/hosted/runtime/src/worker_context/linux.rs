//! Linux AArch64 signal frame extensions from asm/sigcontext.h.

use super::{Exception, NZCV, Registers};
use core::{ffi::c_void, mem, ptr};

const FPSIMD_MAGIC: u32 = 0x4650_8001;
const ESR_MAGIC: u32 = 0x4553_5201;
const RECORD_AREA_LEN: usize = 4096;
const RECORD_AREA_OFFSET: usize =
    (mem::offset_of!(libc::mcontext_t, pstate) + size_of::<u64>()).next_multiple_of(16);

#[repr(C)]
struct RecordHeader {
    magic: u32,
    size: u32,
}

#[repr(C)]
struct FpsimdContext {
    header: RecordHeader,
    fpsr: u32,
    fpcr: u32,
    q: [u128; 32],
}

pub(crate) struct SignalContext<'a> {
    frame: &'a mut libc::mcontext_t,
}

impl SignalContext<'_> {
    /// The pointer must be the live context supplied by the signal trampoline.
    pub(crate) unsafe fn new(raw: *mut c_void) -> Self {
        let context = unsafe { &mut (*raw.cast::<libc::ucontext_t>()).uc_mcontext };
        Self { frame: context }
    }

    pub(crate) unsafe fn exception(
        &mut self,
        signal: i32,
        info: *const libc::siginfo_t,
    ) -> Exception {
        let pc = self.frame.pc;
        let address = self.frame.fault_address;
        let esr = match signal {
            libc::SIGTRAP if unsafe { (*info).si_code } == libc::TRAP_BRKPT => {
                // Linux reports the BRK address as PC but does not export its ESR.
                // A synchronous BRK guarantees the instruction was fetchable.
                let word =
                    unsafe { ptr::with_exposed_provenance::<u32>(pc as usize).read_unaligned() };
                if word & 0xffe0_001f == 0xd420_0000 {
                    (0x3c << 26) | (1 << 25) | ((word >> 5) & 0xffff)
                } else {
                    1 << 25
                }
            }
            libc::SIGSEGV | libc::SIGBUS => {
                let raw = self.esr().unwrap_or(0);
                match raw >> 26 {
                    0x20 | 0x24 => raw,
                    _ if address == pc => (0x20 << 26) | (1 << 25),
                    _ => (0x24 << 26) | (1 << 25),
                }
            }
            libc::SIGUSR1 => 0,
            _ => 1 << 25,
        };
        let registers = self.capture();
        Exception {
            registers,
            esr,
            far: address,
        }
    }

    pub(crate) fn capture(&mut self) -> Registers {
        let ctx = &mut self.frame;
        let mut regs = Registers {
            x: ctx.regs,
            sp: ctx.sp,
            pc: ctx.pc,
            nzcv: (ctx.pstate as u32) & NZCV,
            ..Registers::default()
        };
        let vector = self.fpsimd();
        regs.q = vector.q;
        regs.fpsr = vector.fpsr;
        regs.fpcr = vector.fpcr;
        regs
    }

    pub(crate) fn restore(&mut self, regs: Registers) {
        let ctx = &mut self.frame;
        ctx.regs = regs.x;
        ctx.sp = regs.sp;
        ctx.pc = regs.pc;
        ctx.pstate = (ctx.pstate & !u64::from(NZCV)) | u64::from(regs.nzcv & NZCV);
        let vector = self.fpsimd();
        vector.q = regs.q;
        vector.fpcr = regs.fpcr;
        vector.fpsr = regs.fpsr;
    }

    fn record<T>(&mut self, magic: u32) -> Option<*mut T> {
        let mut offset = 0;
        let records = unsafe {
            ptr::from_mut(&mut *self.frame)
                .cast::<u8>()
                .add(RECORD_AREA_OFFSET)
        };
        while offset + size_of::<RecordHeader>() <= RECORD_AREA_LEN {
            let record = unsafe { records.add(offset) };
            let header = unsafe { &*record.cast::<RecordHeader>() };
            let size = header.size as usize;
            if size < 16 || size % 16 != 0 || size > RECORD_AREA_LEN - offset {
                return None;
            }
            if header.magic == magic {
                return (size >= size_of::<T>()).then(|| record.cast());
            }
            offset += size;
        }
        None
    }

    fn fpsimd(&mut self) -> &mut FpsimdContext {
        let Some(record) = self.record::<FpsimdContext>(FPSIMD_MAGIC) else {
            unsafe { libc::_exit(1) };
        };
        unsafe { &mut *record }
    }

    fn esr(&mut self) -> Option<u32> {
        let record = self.record::<[u64; 2]>(ESR_MAGIC)?;
        Some(unsafe { (*record)[1] as u32 })
    }
}
