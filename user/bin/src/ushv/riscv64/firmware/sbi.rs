use super::{Firmware, FirmwareAction};
use scarlet_std::hypervisor::{Vcpu, arch::reg};

// TODO: SBI v2.0/v3.0 Required Extensions Implementation Status
//
// Extension  | Status      | Notes
// -----------|-------------|----------------------------------
// BASE       | ✅ Done     | All functions implemented
// TIMER      | ⚠️ Stub     | TODO: Program guest timer interrupt via vstimecmp
// IPI        | ⚠️ Stub     | TODO: Inject VS-mode software interrupt via hvip
// RFENCE     | ⚠️ Stub     | TODO: Execute actual fence instructions on target harts
// HSM        | ⚠️ Partial  | TODO: HART_START, HART_SUSPEND need multi-vcpu support
// SRST       | ✅ Done     | Shutdown/reboot works
// DBCN       | ⚠️ Partial  | TODO: WRITE/READ need guest memory access
// PMU        | ❌ Missing  | Optional - performance counters
// STA        | ❌ Missing  | Optional - steal time accounting
// CPPC       | ❌ Missing  | Optional - collaborative processor performance

pub mod eid {
    pub const LEGACY_PUTCHAR: u64 = 0x01;
    pub const LEGACY_GETCHAR: u64 = 0x02;
    pub const BASE: u64 = 0x10;
    pub const TIMER: u64 = 0x54494D45;
    pub const IPI: u64 = 0x735049;
    pub const RFENCE: u64 = 0x52464E43;
    pub const HSM: u64 = 0x48534D;
    pub const SRST: u64 = 0x53525354;
    pub const DBCN: u64 = 0x4442434E;
}

mod fid {
    pub mod base {
        pub const GET_SPEC_VERSION: u64 = 0;
        pub const GET_IMPL_ID: u64 = 1;
        pub const GET_IMPL_VERSION: u64 = 2;
        pub const PROBE_EXTENSION: u64 = 3;
        pub const GET_MVENDORID: u64 = 4;
        pub const GET_MARCHID: u64 = 5;
        pub const GET_MIMPID: u64 = 6;
    }

    pub mod timer {
        pub const SET_TIMER: u64 = 0;
    }

    pub mod ipi {
        pub const SEND_IPI: u64 = 0;
    }

    pub mod rfence {
        pub const REMOTE_FENCE_I: u64 = 0;
        pub const REMOTE_SFENCE_VMA: u64 = 1;
        pub const REMOTE_SFENCE_VMA_ASID: u64 = 2;
        pub const REMOTE_HFENCE_GVMA_VMID: u64 = 3;
        pub const REMOTE_HFENCE_GVMA: u64 = 4;
        pub const REMOTE_HFENCE_VVMA_ASID: u64 = 5;
        pub const REMOTE_HFENCE_VVMA: u64 = 6;
    }

    pub mod hsm {
        pub const HART_START: u64 = 0;
        pub const HART_STOP: u64 = 1;
        pub const HART_GET_STATUS: u64 = 2;
        pub const HART_SUSPEND: u64 = 3;
    }

    pub mod srst {
        pub const SYSTEM_RESET: u64 = 0;
    }

    pub mod dbcn {
        pub const WRITE: u64 = 0;
        pub const READ: u64 = 1;
        pub const WRITE_BYTE: u64 = 2;
    }
}

mod error {
    pub const SUCCESS: i64 = 0;
    pub const FAILED: i64 = -1;
    pub const NOT_SUPPORTED: i64 = -2;
    pub const INVALID_PARAM: i64 = -3;
    pub const DENIED: i64 = -4;
    pub const INVALID_ADDRESS: i64 = -5;
}

mod hsm_state {
    pub const STARTED: u64 = 0;
    pub const STOPPED: u64 = 1;
    pub const START_PENDING: u64 = 2;
    pub const STOP_PENDING: u64 = 3;
}

pub struct SbiFirmware;

impl SbiFirmware {
    pub fn new() -> Self {
        Self
    }
}

impl Firmware for SbiFirmware {
    fn handle(&mut self, vcpu: &mut Vcpu) -> FirmwareAction {
        let extension = vcpu.get_reg(reg::A7).unwrap_or(0);
        let function = vcpu.get_reg(reg::A6).unwrap_or(0);
        let a0 = vcpu.get_reg(reg::A0).unwrap_or(0);
        let a1 = vcpu.get_reg(reg::A1).unwrap_or(0);
        let a2 = vcpu.get_reg(reg::A2).unwrap_or(0);

        let ((error, value), action) = match extension {
            eid::BASE => self.handle_base(function, a0),
            eid::LEGACY_PUTCHAR => self.handle_legacy_putchar(a0),
            eid::LEGACY_GETCHAR => ((error::FAILED, 0), FirmwareAction::Continue),
            eid::TIMER => self.handle_timer(function),
            eid::IPI => self.handle_ipi(function),
            eid::RFENCE => self.handle_rfence(function),
            eid::HSM => self.handle_hsm(function, a0, a1, a2),
            eid::SRST => self.handle_srst(function, a0, a1),
            eid::DBCN => self.handle_dbcn(function, a0, a1, a2),
            _ => ((error::NOT_SUPPORTED, 0), FirmwareAction::Continue),
        };

        let _ = vcpu.set_reg(reg::A0, error as u64);
        let _ = vcpu.set_reg(reg::A1, value);
        action
    }
}

impl SbiFirmware {
    fn handle_base(&mut self, function: u64, a0: u64) -> ((i64, u64), FirmwareAction) {
        let ret = match function {
            fid::base::GET_SPEC_VERSION => (error::SUCCESS, (2 << 24) | 0),
            fid::base::GET_IMPL_ID => (error::SUCCESS, 4),
            fid::base::GET_IMPL_VERSION => (error::SUCCESS, 0),
            fid::base::PROBE_EXTENSION => self.probe_extension(a0),
            fid::base::GET_MVENDORID => (error::SUCCESS, 0),
            fid::base::GET_MARCHID => (error::SUCCESS, 0),
            fid::base::GET_MIMPID => (error::SUCCESS, 0),
            _ => (error::NOT_SUPPORTED, 0),
        };
        (ret, FirmwareAction::Continue)
    }

    fn probe_extension(&self, ext_id: u64) -> (i64, u64) {
        let supported = matches!(
            ext_id,
            eid::BASE | eid::TIMER | eid::IPI | eid::RFENCE | eid::HSM | eid::SRST | eid::DBCN
        );
        (error::SUCCESS, if supported { 1 } else { 0 })
    }

    fn handle_legacy_putchar(&mut self, a0: u64) -> ((i64, u64), FirmwareAction) {
        let ch = a0 as u8 as char;
        scarlet_std::print!("{}", ch);
        ((error::SUCCESS, 0), FirmwareAction::Continue)
    }

    fn handle_timer(&mut self, function: u64) -> ((i64, u64), FirmwareAction) {
        // TODO(TIMER): Program guest timer via vstimecmp CSR
        // - Read stime_value from a0
        // - Set vstimecmp to trigger VS-mode timer interrupt
        match function {
            fid::timer::SET_TIMER => ((error::SUCCESS, 0), FirmwareAction::Continue),
            _ => ((error::NOT_SUPPORTED, 0), FirmwareAction::Continue),
        }
    }

    fn handle_ipi(&mut self, function: u64) -> ((i64, u64), FirmwareAction) {
        // TODO(IPI): Inject VS-mode software interrupt
        // - Parse hart_mask from a0, hart_mask_base from a1
        // - Set VSSIP in hvip for target harts
        match function {
            fid::ipi::SEND_IPI => ((error::SUCCESS, 0), FirmwareAction::Continue),
            _ => ((error::NOT_SUPPORTED, 0), FirmwareAction::Continue),
        }
    }

    fn handle_rfence(&mut self, function: u64) -> ((i64, u64), FirmwareAction) {
        // TODO(RFENCE): Execute actual fence operations
        // - Parse hart_mask, start_addr, size, asid/vmid from args
        // - Execute FENCE.I, SFENCE.VMA, HFENCE.GVMA, HFENCE.VVMA
        match function {
            fid::rfence::REMOTE_FENCE_I
            | fid::rfence::REMOTE_SFENCE_VMA
            | fid::rfence::REMOTE_SFENCE_VMA_ASID
            | fid::rfence::REMOTE_HFENCE_GVMA_VMID
            | fid::rfence::REMOTE_HFENCE_GVMA
            | fid::rfence::REMOTE_HFENCE_VVMA_ASID
            | fid::rfence::REMOTE_HFENCE_VVMA => ((error::SUCCESS, 0), FirmwareAction::Continue),
            _ => ((error::NOT_SUPPORTED, 0), FirmwareAction::Continue),
        }
    }

    fn handle_hsm(
        &mut self,
        function: u64,
        a0: u64,
        _a1: u64,
        _a2: u64,
    ) -> ((i64, u64), FirmwareAction) {
        // TODO(HSM): Full hart state management
        // - HART_START: Create/start new vcpu thread (need multi-vcpu support)
        // - HART_STOP: Stop current vcpu
        // - HART_SUSPEND: Platform-specific suspend states
        match function {
            fid::hsm::HART_START => {
                if a0 == 0 {
                    ((error::INVALID_PARAM, 0), FirmwareAction::Continue)
                } else {
                    ((error::NOT_SUPPORTED, 0), FirmwareAction::Continue)
                }
            }
            fid::hsm::HART_STOP => ((error::SUCCESS, 0), FirmwareAction::Continue),
            fid::hsm::HART_GET_STATUS => (
                (error::SUCCESS, hsm_state::STARTED),
                FirmwareAction::Continue,
            ),
            fid::hsm::HART_SUSPEND => ((error::NOT_SUPPORTED, 0), FirmwareAction::Continue),
            _ => ((error::NOT_SUPPORTED, 0), FirmwareAction::Continue),
        }
    }

    fn handle_srst(
        &mut self,
        function: u64,
        reset_type: u64,
        _reset_reason: u64,
    ) -> ((i64, u64), FirmwareAction) {
        match function {
            fid::srst::SYSTEM_RESET => match reset_type {
                0 | 1 | 2 => ((error::SUCCESS, 0), FirmwareAction::Shutdown),
                _ => ((error::NOT_SUPPORTED, 0), FirmwareAction::Continue),
            },
            _ => ((error::NOT_SUPPORTED, 0), FirmwareAction::Continue),
        }
    }

    fn handle_dbcn(
        &mut self,
        function: u64,
        a0: u64,
        _a1: u64,
        _a2: u64,
    ) -> ((i64, u64), FirmwareAction) {
        // TODO(DBCN): Guest memory access for WRITE/READ
        // - WRITE: Read bytes from guest physical memory (a1=addr_lo, a2=addr_hi)
        // - READ: Write bytes to guest physical memory
        match function {
            fid::dbcn::WRITE_BYTE => {
                let ch = a0 as u8 as char;
                scarlet_std::print!("{}", ch);
                ((error::SUCCESS, 0), FirmwareAction::Continue)
            }
            fid::dbcn::WRITE | fid::dbcn::READ => {
                ((error::NOT_SUPPORTED, 0), FirmwareAction::Continue)
            }
            _ => ((error::NOT_SUPPORTED, 0), FirmwareAction::Continue),
        }
    }
}

impl Default for SbiFirmware {
    fn default() -> Self {
        Self::new()
    }
}
