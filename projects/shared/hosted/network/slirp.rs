//! IPv4 user-mode NAT. Only this host thread calls libslirp.

use super::{Carrier, HEADER, MTU};
use scarlet::device::network::DevicePacket;
use std::{
    cell::Cell,
    ffi::{CStr, CString, c_char, c_int, c_void},
    io::{self, Read},
    os::{
        fd::AsRawFd,
        unix::{ffi::OsStrExt, net::UnixStream},
    },
    path::{Path, PathBuf},
    ptr::{self, NonNull},
    sync::{Arc, atomic::Ordering, mpsc},
    time::Instant,
};

// Version 1 of the public SlirpConfig ABI. Keep field order from libslirp.h.
// IPv6, host loopback, TFTP and protocol helpers are disabled.
#[repr(C)]
struct Config {
    version: u32,
    restricted: c_int,
    ipv4: bool,
    network: libc::in_addr,
    netmask: libc::in_addr,
    host: libc::in_addr,
    ipv6: bool,
    prefix6: libc::in6_addr,
    prefix_len: u8,
    host6: libc::in6_addr,
    hostname: *const c_char,
    tftp_server_name: *const c_char,
    tftp_path: *const c_char,
    bootfile: *const c_char,
    dhcp_start: libc::in_addr,
    dns: libc::in_addr,
    dns6: libc::in6_addr,
    dns_search: *const *const c_char,
    domain: *const c_char,
    mtu: usize,
    mru: usize,
    disable_host_loopback: bool,
    enable_emu: bool,
}

impl Config {
    fn ipv4() -> Self {
        fn addr(octets: [u8; 4]) -> libc::in_addr {
            libc::in_addr {
                s_addr: u32::from_ne_bytes(octets),
            }
        }

        // Zero is valid for the remaining booleans, integers and C pointers.
        Self {
            version: 1,
            ipv4: true,
            network: addr([10, 0, 2, 0]),
            netmask: addr([255, 255, 255, 0]),
            host: addr([10, 0, 2, 2]),
            dhcp_start: addr([10, 0, 2, 15]),
            dns: addr([10, 0, 2, 3]),
            mtu: super::MTU,
            mru: super::MTU,
            disable_host_loopback: true,
            ..unsafe { std::mem::zeroed() }
        }
    }
}

type PollCallback = unsafe extern "C" fn(c_int, c_int, *mut c_void) -> c_int;
type ReventsCallback = unsafe extern "C" fn(c_int, *mut c_void) -> c_int;
type TimerCallback = unsafe extern "C" fn(*mut c_void);
type FdCallback = unsafe extern "C" fn(c_int, *mut c_void);
type Slirp = *mut c_void;

#[repr(C)]
struct Callbacks {
    send_packet: unsafe extern "C" fn(*const u8, usize, *mut c_void) -> isize,
    guest_error: unsafe extern "C" fn(*const c_char, *mut c_void),
    clock_get_ns: extern "C" fn(*mut c_void) -> i64,
    timer_new: Option<unsafe extern "C" fn(TimerCallback, *mut c_void, *mut c_void) -> *mut c_void>,
    timer_free: Option<unsafe extern "C" fn(*mut c_void, *mut c_void)>,
    timer_mod: Option<unsafe extern "C" fn(*mut c_void, i64, *mut c_void)>,
    register_poll_fd: FdCallback,
    unregister_poll_fd: FdCallback,
    notify: extern "C" fn(*mut c_void),
    init_completed: Option<unsafe extern "C" fn(Slirp, *mut c_void)>,
    timer_new_opaque: Option<unsafe extern "C" fn(c_int, *mut c_void, *mut c_void) -> *mut c_void>,
    register_poll_socket: FdCallback,
    unregister_poll_socket: FdCallback,
}

static CALLBACKS: Callbacks = Callbacks {
    send_packet: receive,
    guest_error,
    clock_get_ns: clock_get_ns,
    timer_new: None,
    timer_free: None,
    timer_mod: None,
    register_poll_fd: close_on_exec,
    unregister_poll_fd: ignore_fd,
    notify: notify,
    init_completed: None,
    timer_new_opaque: None,
    register_poll_socket: close_on_exec,
    unregister_poll_socket: ignore_fd,
};

const POLL_FLAGS: [(c_int, i16); 5] = [
    (1 << 0, libc::POLLIN),
    (1 << 1, libc::POLLOUT),
    (1 << 2, libc::POLLPRI),
    (1 << 3, libc::POLLERR),
    (1 << 4, libc::POLLHUP),
];

unsafe extern "C" fn add_poll(fd: c_int, events: c_int, opaque: *mut c_void) -> c_int {
    let fds = unsafe { &mut *opaque.cast::<Vec<libc::pollfd>>() };
    let index = fds.len() as c_int;
    let events = POLL_FLAGS
        .iter()
        .filter(|(slirp, _)| events & slirp != 0)
        .fold(0, |flags, &(_, posix)| flags | posix);
    fds.push(libc::pollfd {
        fd,
        events,
        revents: 0,
    });
    index
}

unsafe extern "C" fn revents(index: c_int, opaque: *mut c_void) -> c_int {
    let fds = unsafe { &*opaque.cast::<Vec<libc::pollfd>>() };
    let Some(entry) = fds.get(index as usize) else {
        return 0;
    };
    let events = entry.revents;
    POLL_FLAGS
        .iter()
        .filter(|(_, posix)| events & posix != 0)
        .fold(0, |flags, &(slirp, _)| flags | slirp)
}

struct Library(NonNull<c_void>);

impl Library {
    fn open(path: &CStr) -> Result<Self, String> {
        let handle = unsafe { libc::dlopen(path.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL) };
        NonNull::new(handle).map(Self).ok_or_else(dl_error)
    }

    fn symbol(&self, name: &CStr) -> Result<*mut c_void, String> {
        let symbol = unsafe { libc::dlsym(self.0.as_ptr(), name.as_ptr()) };
        NonNull::new(symbol)
            .map(NonNull::as_ptr)
            .ok_or_else(dl_error)
    }
}

impl Drop for Library {
    fn drop(&mut self) {
        unsafe { libc::dlclose(self.0.as_ptr()) };
    }
}

fn dl_error() -> String {
    let error = unsafe { libc::dlerror() };
    if error.is_null() {
        "dynamic loader did not report an error".into()
    } else {
        unsafe { CStr::from_ptr(error) }
            .to_string_lossy()
            .into_owned()
    }
}

const TX_BUDGET: usize = 64;

struct Context {
    rx: mpsc::SyncSender<DevicePacket>,
    carrier: Arc<Carrier>,
    pending: Cell<bool>,
    start: Instant,
}

extern "C" fn clock_get_ns(opaque: *mut c_void) -> i64 {
    let context = unsafe { &*opaque.cast::<Context>() };
    context.start.elapsed().as_nanos().min(i64::MAX as u128) as i64
}

unsafe extern "C" fn ignore_fd(_: c_int, _: *mut c_void) {}

extern "C" fn notify(_: *mut c_void) {
    // This transport only enters libslirp from the I/O thread.
}

unsafe extern "C" fn receive(data: *const u8, len: usize, opaque: *mut c_void) -> isize {
    // The Context remains allocated through slirp_cleanup, which may invoke
    // callbacks itself. The frame data belongs to libslirp for this call.
    let context = unsafe { &*opaque.cast::<Context>() };
    if data.is_null() || !(HEADER..=HEADER + MTU).contains(&len) {
        context.carrier.rx_errors.fetch_add(1, Ordering::Relaxed);
        return 0;
    }
    let frame = unsafe { std::slice::from_raw_parts(data, len) };
    if context
        .rx
        .try_send(DevicePacket::with_data(frame.to_vec()))
        .is_err()
    {
        context.carrier.dropped.fetch_add(1, Ordering::Relaxed);
        return 0;
    }
    context.carrier.rx_packets.fetch_add(1, Ordering::Relaxed);
    context
        .carrier
        .rx_bytes
        .fetch_add(len as u64, Ordering::Relaxed);
    context.pending.set(true);
    len as isize
}

unsafe extern "C" fn guest_error(_: *const c_char, opaque: *mut c_void) {
    let context = unsafe { &*opaque.cast::<Context>() };
    context.carrier.tx_errors.fetch_add(1, Ordering::Relaxed);
}

unsafe extern "C" fn close_on_exec(fd: c_int, _: *mut c_void) {
    if unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) } < 0 {
        // The callback cannot return an error. Do not leave a host socket
        // inheritable by a worker created after this point.
        std::process::abort();
    }
}

struct Nat {
    slirp: Slirp,
    cleanup: unsafe extern "C" fn(Slirp),
    input: unsafe extern "C" fn(Slirp, *const u8, c_int),
    fill: unsafe extern "C" fn(Slirp, *mut u32, PollCallback, *mut c_void),
    poll: unsafe extern "C" fn(Slirp, c_int, ReventsCallback, *mut c_void),
    context: Box<Context>,
    fds: Vec<libc::pollfd>,
    _library: Library,
}

impl Nat {
    fn new(path: &Path, context: Box<Context>) -> Result<Self, String> {
        let native_path = CString::new(path.as_os_str().as_bytes())
            .map_err(|_| "Invalid libslirp path")?;
        let library = Library::open(&native_path)
            .map_err(|error| format!("Cannot load libslirp ({}): {error}", path.display()))?;
        // Names and signatures match the version 1 libslirp ABI.
        let new: unsafe extern "C" fn(*const Config, *const Callbacks, *mut c_void) -> Slirp =
            unsafe { std::mem::transmute(library.symbol(c"slirp_new")?) };
        let cleanup = unsafe { std::mem::transmute(library.symbol(c"slirp_cleanup")?) };
        let input = unsafe { std::mem::transmute(library.symbol(c"slirp_input")?) };
        let fill = unsafe { std::mem::transmute(library.symbol(c"slirp_pollfds_fill")?) };
        let poll = unsafe { std::mem::transmute(library.symbol(c"slirp_pollfds_poll")?) };
        let opaque = ptr::from_ref(context.as_ref()).cast_mut().cast();
        let slirp = unsafe { new(&Config::ipv4(), &CALLBACKS, opaque) };
        if slirp.is_null() {
            return Err("libslirp rejected IPv4 configuration".into());
        }
        Ok(Self {
            slirp,
            cleanup,
            input,
            fill,
            poll,
            context,
            fds: Vec::new(),
            _library: library,
        })
    }

    fn serve(&mut self, tx: mpsc::Receiver<DevicePacket>, mut wake: UnixStream) -> io::Result<()> {
        loop {
            let mut sent = 0;
            let carrier = &self.context.carrier;
            for frame in tx.try_iter().take(TX_BUDGET) {
                unsafe { (self.input)(self.slirp, frame.data.as_ptr(), frame.len as c_int) };
                carrier.tx_packets.fetch_add(1, Ordering::Relaxed);
                carrier
                    .tx_bytes
                    .fetch_add(frame.len as u64, Ordering::Relaxed);
                sent += 1;
            }
            if sent != 0 {
                carrier.tx_ready.store(true, Ordering::Release);
            }
            if self.context.pending.replace(false) || sent != 0 {
                carrier.signal();
            }
            if !self.wait(&mut wake, sent == TX_BUDGET)? {
                return Ok(());
            }
        }
    }

    fn wait(&mut self, wake: &mut UnixStream, busy: bool) -> io::Result<bool> {
        self.fds.clear();
        self.fds.push(libc::pollfd {
            fd: wake.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        });
        let mut timeout = if busy { 0 } else { u32::MAX };
        let opaque = ptr::from_mut(&mut self.fds).cast();
        unsafe { (self.fill)(self.slirp, &mut timeout, add_poll, opaque) };
        let timeout = if timeout == u32::MAX {
            -1
        } else {
            timeout.min(i32::MAX as u32) as c_int
        };
        let ready = unsafe { libc::poll(self.fds.as_mut_ptr(), self.fds.len() as _, timeout) };
        let error = (ready < 0).then(io::Error::last_os_error);
        unsafe { (self.poll)(self.slirp, c_int::from(error.is_some()), revents, opaque) };
        if let Some(error) = error {
            return if error.kind() == io::ErrorKind::Interrupted {
                Ok(true)
            } else {
                Err(error)
            };
        }

        let events = self.fds[0].revents;
        if events & (libc::POLLHUP | libc::POLLERR | libc::POLLNVAL) != 0 {
            return Ok(false);
        }
        if events & libc::POLLIN != 0 {
            match wake.read(&mut [0; 256]) {
                Ok(0) => return Ok(false),
                Ok(_) => {}
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::Interrupted | io::ErrorKind::WouldBlock
                    ) => {}
                Err(error) => return Err(error),
            }
        }
        Ok(true)
    }
}

impl Drop for Nat {
    fn drop(&mut self) {
        unsafe { (self.cleanup)(self.slirp) };
    }
}

pub(super) fn start(
    library: PathBuf,
    carrier: Arc<Carrier>,
    tx: mpsc::Receiver<DevicePacket>,
    rx: mpsc::SyncSender<DevicePacket>,
    wake: UnixStream,
) -> Result<(), String> {
    let (ready, result) = mpsc::sync_channel(1);
    std::thread::Builder::new()
        .name("scarlet-ethernet".into())
        .spawn(move || {
            let context = Box::new(Context {
                carrier: carrier.clone(),
                rx,
                pending: Cell::new(false),
                start: Instant::now(),
            });
            let mut nat = match Nat::new(&library, context) {
                Ok(nat) => nat,
                Err(error) => {
                    let _ = ready.send(Err(error));
                    return;
                }
            };
            if ready.send(Ok(())).is_ok() {
                if let Err(error) = nat.serve(tx, wake) {
                    eprintln!("scarlet: hosted Ethernet failed: {error}");
                }
                carrier.up.store(false, Ordering::Release);
                carrier.tx_ready.store(true, Ordering::Release);
                carrier.signal();
            }
        })
        .map_err(|error| error.to_string())?;
    result
        .recv()
        .map_err(|_| "Host Ethernet service exited while opening")?
}
