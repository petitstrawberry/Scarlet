//! Owning, serialized wrapper around upstream wpa_supplicant's RSN state machine.
use alloc::boxed::Box;
use core::{
    ffi::{c_int, c_void},
    ptr::NonNull,
    sync::atomic::{AtomicBool, Ordering},
};
use zeroize::Zeroizing;

/// WPA2-Personal / CCMP only, no PMF or PMKID cache. This is the association IE;
/// the actual AP IE must separately come from the driver, never from this value.
pub const WPA2_PSK_CCMP_IE: [u8; 22] = [
    48, 20, 1, 0, 0, 15, 172, 4, 1, 0, 0, 15, 172, 4, 1, 0, 0, 15, 172, 2, 0, 0,
];
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    InvalidProfile,
    EntropyUnavailable,
    Driver,
    Rejected,
    Timeout,
    ScopeLost,
}
/// Driver callbacks cannot reenter any Engine. A successful key callback means
/// committed installation, not submission. An authorized group-key callback must
/// atomically replace old GTK slots without resetting PTK/PN or the data grant.
/// All failures must close ordinary data.
pub trait Adapter {
    fn random(&mut self, output: &mut [u8]) -> Result<(), Error>;
    fn now_ns(&self) -> u64;
    fn send(&mut self, peer: [u8; 6], pdu: &[u8]) -> Result<(), Error>;
    fn key(
        &mut self,
        kind: u32,
        peer: [u8; 6],
        index: u32,
        sequence: &[u8],
        key: &[u8],
    ) -> Result<(), Error>;
    fn set_authorized(&mut self, authorized: bool) -> Result<(), Error>;
    fn abort(&mut self);
    fn require_protected_group(&self) -> bool {
        false
    }
    fn check(&mut self) -> Result<(), Error> {
        Ok(())
    }
}
#[repr(C)]
struct Callbacks {
    ctx: *mut c_void,
    random: unsafe extern "C" fn(*mut c_void, *mut u8, usize) -> c_int,
    now: unsafe extern "C" fn(*mut c_void) -> u64,
    send: unsafe extern "C" fn(*mut c_void, *const u8, *const u8, usize) -> c_int,
    key: unsafe extern "C" fn(
        *mut c_void,
        c_int,
        *const u8,
        c_int,
        *const u8,
        usize,
        *const u8,
        usize,
    ) -> c_int,
    state: unsafe extern "C" fn(*mut c_void, c_int) -> c_int,
    fail: unsafe extern "C" fn(*mut c_void),
}
unsafe extern "C" {
    fn swpa_new(
        cb: *const Callbacks,
        own: *const u8,
        peer: *const u8,
        ssid: *const u8,
        ssid_len: usize,
        pmk: *const u8,
        own_ie: *const u8,
        own_len: usize,
        ap_ie: *const u8,
        ap_len: usize,
    ) -> *mut c_void;
    fn swpa_rx(
        engine: *mut c_void,
        peer: *const u8,
        pdu: *const u8,
        len: usize,
        protected: c_int,
    ) -> c_int;
    fn swpa_tick(engine: *mut c_void) -> c_int;
    fn swpa_next_deadline(engine: *mut c_void) -> u64;
    fn swpa_deauth_reason(engine: *mut c_void) -> u32;
    fn swpa_free(engine: *mut c_void);
}
static ENTERED: AtomicBool = AtomicBool::new(false);
struct CallGuard;
impl CallGuard {
    fn enter() -> Self {
        while ENTERED
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            core::hint::spin_loop();
        }
        Self
    }
}
impl Drop for CallGuard {
    fn drop(&mut self) {
        ENTERED.store(false, Ordering::Release);
    }
}
struct Context<A> {
    adapter: A,
    error: Option<Error>,
    authorized: bool,
}
impl<A: Adapter> Context<A> {
    fn failed(&mut self, error: Error) {
        if self.error.is_none() {
            self.error = Some(error);
            self.authorized = false;
            self.adapter.abort();
        }
    }
}
unsafe fn context<'a, A>(raw: *mut c_void) -> &'a mut Context<A> {
    unsafe { &mut *raw.cast::<Context<A>>() }
}
unsafe fn slice<'a>(ptr: *const u8, len: usize) -> &'a [u8] {
    if len == 0 {
        &[]
    } else {
        unsafe { core::slice::from_raw_parts(ptr, len) }
    }
}
unsafe extern "C" fn random<A: Adapter>(ctx: *mut c_void, buf: *mut u8, len: usize) -> c_int {
    let c = unsafe { context::<A>(ctx) };
    let out = unsafe { core::slice::from_raw_parts_mut(buf, len) };
    if c.error.is_some() {
        return -1;
    }
    match c.adapter.random(out) {
        Ok(()) => 0,
        Err(_) => {
            use zeroize::Zeroize;
            out.zeroize();
            c.failed(Error::EntropyUnavailable);
            -1
        }
    }
}
unsafe extern "C" fn now<A: Adapter>(ctx: *mut c_void) -> u64 {
    unsafe { context::<A>(ctx) }.adapter.now_ns()
}
unsafe extern "C" fn send<A: Adapter>(
    ctx: *mut c_void,
    peer: *const u8,
    pdu: *const u8,
    len: usize,
) -> c_int {
    let c = unsafe { context::<A>(ctx) };
    if c.error.is_some() {
        return -1;
    }
    let peer = unsafe { *peer.cast::<[u8; 6]>() };
    match c.adapter.send(peer, unsafe { slice(pdu, len) }) {
        Ok(()) => 0,
        Err(e) => {
            c.failed(e);
            -1
        }
    }
}
unsafe extern "C" fn key<A: Adapter>(
    ctx: *mut c_void,
    kind: c_int,
    peer: *const u8,
    index: c_int,
    sequence: *const u8,
    seq_len: usize,
    value: *const u8,
    len: usize,
) -> c_int {
    let c = unsafe { context::<A>(ctx) };
    if c.error.is_some() {
        return -1;
    }
    if !(0..=2).contains(&kind)
        || index < 0
        || seq_len > 6
        || (kind != 0 && len != 16)
        || peer.is_null()
    {
        c.failed(Error::Driver);
        return -1;
    }
    let peer = unsafe { *peer.cast::<[u8; 6]>() };
    match c.adapter.key(
        kind as u32,
        peer,
        index as u32,
        unsafe { slice(sequence, seq_len) },
        unsafe { slice(value, len) },
    ) {
        Ok(()) => 0,
        Err(e) => {
            c.failed(e);
            -1
        }
    }
}
unsafe extern "C" fn state<A: Adapter>(ctx: *mut c_void, authorized: c_int) -> c_int {
    let c = unsafe { context::<A>(ctx) };
    if c.error.is_some() {
        return -1;
    }
    let authorized = authorized == 1;
    if c.authorized == authorized {
        return 0;
    }
    match c.adapter.set_authorized(authorized) {
        Ok(()) => {
            c.authorized = authorized;
            0
        }
        Err(e) => {
            c.failed(e);
            -1
        }
    }
}
unsafe extern "C" fn failed<A: Adapter>(ctx: *mut c_void) {
    unsafe { context::<A>(ctx) }.failed(Error::Rejected);
}

/// One instance per association. Drop aborts Native authorization and wipes C
/// allocations through the port allocator. Raw state is deliberately not Send/Sync.
pub struct Engine<A: Adapter> {
    raw: NonNull<c_void>,
    ctx: Box<Context<A>>,
    peer: [u8; 6],
    deadline: u64,
}
impl<A: Adapter> Engine<A> {
    pub fn new(
        mut adapter: A,
        own: [u8; 6],
        peer: [u8; 6],
        ssid: &[u8],
        passphrase: &str,
        ap_ie: &[u8],
        timeout_ns: u64,
    ) -> Result<Self, Error> {
        let pmk = match crate::derive_pmk(passphrase, ssid) {
            Ok(pmk) => Zeroizing::new(pmk),
            Err(_) => {
                adapter.abort();
                return Err(Error::InvalidProfile);
            }
        };
        Self::from_pmk(adapter, own, peer, ssid, &pmk, ap_ie, timeout_ns)
    }
    pub fn from_pmk(
        mut adapter: A,
        own: [u8; 6],
        peer: [u8; 6],
        ssid: &[u8],
        pmk: &[u8; 32],
        ap_ie: &[u8],
        timeout_ns: u64,
    ) -> Result<Self, Error> {
        if ssid.is_empty()
            || ssid.len() > 32
            || own[0] & 1 != 0
            || peer[0] & 1 != 0
            || own == [0; 6]
            || peer == [0; 6]
            || own == peer
            || timeout_ns == 0
        {
            adapter.abort();
            return Err(Error::InvalidProfile);
        }
        if let Err(e) = adapter.check().and_then(|_| adapter.set_authorized(false)) {
            adapter.abort();
            return Err(e);
        }
        let Some(deadline) = adapter.now_ns().checked_add(timeout_ns) else {
            adapter.abort();
            return Err(Error::InvalidProfile);
        };
        let mut ctx = Box::new(Context {
            adapter,
            error: None,
            authorized: false,
        });
        let callbacks = Callbacks {
            ctx: (&mut *ctx as *mut Context<A>).cast(),
            random: random::<A>,
            now: now::<A>,
            send: send::<A>,
            key: key::<A>,
            state: state::<A>,
            fail: failed::<A>,
        };
        let _guard = CallGuard::enter();
        let ptr = unsafe {
            swpa_new(
                &callbacks,
                own.as_ptr(),
                peer.as_ptr(),
                ssid.as_ptr(),
                ssid.len(),
                pmk.as_ptr(),
                WPA2_PSK_CCMP_IE.as_ptr(),
                WPA2_PSK_CCMP_IE.len(),
                ap_ie.as_ptr(),
                ap_ie.len(),
            )
        };
        let Some(raw) = NonNull::new(ptr) else {
            ctx.adapter.abort();
            return Err(Error::InvalidProfile);
        };
        if let Some(e) = ctx.error {
            unsafe { swpa_free(raw.as_ptr()) };
            return Err(e);
        }
        Ok(Self {
            raw,
            ctx,
            peer,
            deadline,
        })
    }
    pub fn adapter(&self) -> &A {
        &self.ctx.adapter
    }
    pub fn adapter_mut(&mut self) -> &mut A {
        &mut self.ctx.adapter
    }
    pub fn authorized(&self) -> bool {
        self.ctx.authorized && self.ctx.error.is_none()
    }
    /// Earliest upstream protocol timer or initial authentication deadline.
    /// An authorized idle connection without timers may wait indefinitely.
    pub fn next_deadline_ns(&self) -> Option<u64> {
        let _guard = CallGuard::enter();
        let timer = unsafe { swpa_next_deadline(self.raw.as_ptr()) };
        let deadline = if self.authorized() {
            timer
        } else {
            timer.min(self.deadline)
        };
        (deadline != u64::MAX).then_some(deadline)
    }
    /// IEEE 802.11 reason supplied by the supplicant on protocol rejection.
    pub fn deauth_reason(&self) -> u32 {
        unsafe { swpa_deauth_reason(self.raw.as_ptr()) }
    }
    pub fn tick(&mut self) -> Result<(), Error> {
        if let Some(e) = self.ctx.error {
            return Err(e);
        }
        if let Err(e) = self.ctx.adapter.check() {
            self.ctx.failed(e);
            return Err(e);
        }
        if !self.authorized() && self.ctx.adapter.now_ns() >= self.deadline {
            self.ctx.failed(Error::Timeout);
            return Err(Error::Timeout);
        }
        let result = {
            let _guard = CallGuard::enter();
            unsafe { swpa_tick(self.raw.as_ptr()) }
        };
        if result < 0 && self.ctx.error.is_none() {
            self.ctx.failed(Error::Rejected);
        }
        self.ctx.error.map_or(Ok(()), Err)
    }
    pub fn receive(&mut self, peer: [u8; 6], pdu: &[u8]) -> Result<bool, Error> {
        self.receive_protected(peer, pdu, false)
    }
    /// `protected` is trusted driver metadata after CCMP/MIC/PN verification.
    pub fn receive_protected(
        &mut self,
        peer: [u8; 6],
        pdu: &[u8],
        protected: bool,
    ) -> Result<bool, Error> {
        self.tick()?;
        if peer != self.peer {
            return Ok(false);
        }
        if pdu.len() < 4
            || pdu.len() > 1024
            || !(1..=3).contains(&pdu[0])
            || pdu[1] != 3
            || usize::from(u16::from_be_bytes([pdu[2], pdu[3]])) + 4 != pdu.len()
        {
            self.ctx.failed(Error::Rejected);
            return Err(Error::Rejected);
        }
        // Do not let an unauthenticated plaintext downgrade tear down or rotate
        // the live GTK. Pairwise M3 retransmission remains upstream validated.
        if self.authorized()
            && self.ctx.adapter.require_protected_group()
            && !protected
            && pdu.len() >= 7
            && u16::from_be_bytes([pdu[5], pdu[6]]) & 8 == 0
        {
            return Ok(false);
        }
        let result = {
            let _guard = CallGuard::enter();
            unsafe {
                swpa_rx(
                    self.raw.as_ptr(),
                    peer.as_ptr(),
                    pdu.as_ptr(),
                    pdu.len(),
                    protected as c_int,
                )
            }
        };
        if let Some(e) = self.ctx.error {
            return Err(e);
        }
        if result == -2 {
            return Ok(false);
        }
        if result < 0 {
            self.ctx.failed(Error::Rejected);
            return Err(Error::Rejected);
        }
        Ok(result == 1)
    }
}
impl<A: Adapter> Drop for Engine<A> {
    fn drop(&mut self) {
        self.ctx.adapter.abort();
        let _guard = CallGuard::enter();
        unsafe { swpa_free(self.raw.as_ptr()) };
    }
}
