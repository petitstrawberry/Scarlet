//! Only owned command data and IDs cross from Scarlet to the native event loop.
use super::*;
use scarlet::{
    device::graphics::{PresentationOwners, PresentationWait, output::DisplayRegion},
    interrupt::{InterruptClaim, InterruptManager, InterruptResult, InterruptSource},
    sync::{IrqSpinLock, Waker},
};
use std::{
    collections::VecDeque,
    sync::{
        OnceLock,
        atomic::{AtomicBool, AtomicU64},
        mpsc,
    },
};

const IRQ: u32 = 22;
const CAPACITY: usize = 32;
pub(super) type ResultBytes = Result<Vec<u8>, &'static str>;

pub(super) enum Command {
    CreateImage(u64, u32, u32),
    DropImage(u64),
    CreateQueue(u64),
    DropQueue(u64),
    Execute {
        queue: u64,
        words: Vec<u64>,
        attachments: Arc<BTreeMap<u64, u32>>,
    },
    Upload(u64, Rect, Vec<u8>),
    Readback(u64, Rect),
    Present(u64, Rect),
}
#[derive(Clone, Copy)]
pub(super) struct Rect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}
pub(super) struct Request {
    pub id: u64,
    pub command: Command,
}
enum Reply {
    Done(u64, ResultBytes),
    Closed,
}
#[derive(Default)]
pub(super) struct Inbox(Mutex<VecDeque<Reply>>);
impl Inbox {
    fn post(&self, reply: Reply) {
        self.0.lock().unwrap().push_back(reply);
        let _ = scarlet::hosted::raise_external_interrupt(0, IRQ);
    }
    pub fn done(&self, id: u64, result: ResultBytes) {
        if id != 0 {
            self.post(Reply::Done(id, result));
        }
    }
    pub fn close(&self) {
        self.post(Reply::Closed);
    }
}

struct Ticket {
    result: IrqSpinLock<Option<ResultBytes>>,
    ready: Arc<Waker>,
}
impl Ticket {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            result: IrqSpinLock::new(None),
            ready: Arc::new(Waker::new_uninterruptible("sgfx-request")),
        })
    }
    fn take(&self) -> ResultBytes {
        let task = scarlet::task::mytask().ok_or("SGFX requires a scheduled task")?;
        loop {
            if let Some(result) = self.result.lock().take() {
                return result;
            }
            self.ready
                .clone()
                .wait_result_until_owned(task.get_id(), task.get_trapframe(), || {
                    self.result.lock().is_some()
                });
        }
    }
}
struct PresentTicket(Arc<Ticket>);
impl PresentationWait for PresentTicket {
    fn wait(self: Box<Self>) -> Result<(), &'static str> {
        self.0.take().map(drop)
    }
}
enum Owner {
    Call(Arc<Ticket>),
    Present(Arc<Ticket>, PresentationOwners),
    Submission(GpuSubmission),
}
impl Owner {
    fn retire(self, reply: ResultBytes) {
        let ticket = match self {
            Self::Submission(s) => {
                if reply.is_ok() {
                    s.complete();
                } else {
                    s.retire_failed(GpuCompletionFailure::Execution);
                }
                return;
            }
            Self::Present(ticket, owners) => {
                drop(owners);
                ticket
            }
            Self::Call(ticket) => ticket,
        };
        *ticket.result.lock() = Some(reply);
        ticket.ready.wake_all();
    }
}
#[derive(Default)]
struct Admission {
    pending: BTreeMap<u64, Owner>,
    cpu_access: bool,
    closed: bool,
}

pub(super) struct Driver {
    tx: mpsc::Sender<Request>,
    wake: Box<dyn Fn() + Send + Sync>,
    pub(super) presentation: bool,
    admission: IrqSpinLock<Admission>,
    changed: Arc<Waker>,
    inbox: Arc<Inbox>,
    worker: Arc<Waker>,
    next: AtomicU64,
    pub(super) images: Mutex<BTreeMap<u64, Weak<Image>>>,
}

#[derive(Clone)]
pub struct Endpoint(pub(super) Arc<Driver>);
static ACTIVE: OnceLock<Endpoint> = OnceLock::new();
static STARTED: AtomicBool = AtomicBool::new(false);
struct Irq(Arc<Waker>);
impl InterruptSource for Irq {
    fn interrupt_id(&self) -> Option<u32> {
        Some(IRQ)
    }
    fn claim_interrupt(&self) -> InterruptResult<InterruptClaim> {
        self.0.wake_one();
        Ok(InterruptClaim::Handled)
    }
}

fn completions() {
    let d = &ACTIVE.get().unwrap().0;
    let task = scarlet::task::mytask().unwrap();
    loop {
        let reply = d.inbox.0.lock().unwrap().pop_front();
        match reply {
            Some(Reply::Done(id, result)) => {
                let owner = d.admission.lock().pending.remove(&id);
                if let Some(owner) = owner {
                    owner.retire(result);
                }
                d.changed.wake_all();
            }
            Some(Reply::Closed) => {
                let pending = {
                    let mut state = d.admission.lock();
                    state.closed = true;
                    std::mem::take(&mut state.pending)
                };
                for (_, owner) in pending {
                    owner.retire(Err("Metal event loop closed"));
                }
                d.changed.wake_all();
                return;
            }
            None => {
                d.worker.clone().wait_result_until_owned(
                    task.get_id(),
                    task.get_trapframe(),
                    || !d.inbox.0.lock().unwrap().is_empty(),
                );
            }
        }
    }
}
impl Endpoint {
    pub(super) fn pair(
        presentation: bool,
        wake: impl Fn() + Send + Sync + 'static,
    ) -> (Self, mpsc::Receiver<Request>, Arc<Inbox>) {
        let (tx, rx) = mpsc::channel();
        let inbox = Arc::new(Inbox::default());
        let endpoint = Self(Arc::new(Driver {
            tx,
            inbox: inbox.clone(),
            wake: Box::new(wake),
            presentation,
            admission: IrqSpinLock::new(Admission::default()),
            changed: Arc::new(Waker::new_uninterruptible("sgfx-admission")),
            worker: Arc::new(Waker::new_uninterruptible("sgfx-completion")),
            images: Mutex::default(),
            next: AtomicU64::new(1),
        }));
        (endpoint, rx, inbox)
    }
    pub fn register(&self) -> Result<(), &'static str> {
        let controller = InterruptManager::global();
        let source: Arc<dyn InterruptSource> = Arc::new(Irq(self.0.worker.clone()));
        controller
            .register_interrupt_source(IRQ, source.clone())
            .map_err(|_| "Cannot register SGFX IRQ")?;
        if controller.enable_external_interrupt(IRQ, 0).is_err() {
            controller.unregister_interrupt_source(IRQ, &source);
            return Err("Cannot enable SGFX IRQ");
        }
        ACTIVE
            .set(self.clone())
            .map_err(|_| "SGFX already registered")?;
        register_gpu_control_device(Arc::new(self.clone()))?;
        Ok(())
    }
    pub(super) fn id(&self) -> Result<u64, &'static str> {
        self.0
            .next
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
            .map_err(|_| "SGFX IDs exhausted")
    }
    pub(super) fn cookie(&self) -> u64 {
        Arc::as_ptr(&self.0) as u64
    }
    fn start_worker(&self) {
        if !STARTED.swap(true, Ordering::AcqRel) {
            let task = scarlet::task::new_kernel_task("sgfx-completion".into(), 1, completions);
            task.init();
            scarlet::sched::scheduler::add_task(task, 0);
        }
    }
    // Linearize ID allocation, pending ownership and publication for all vCPUs.
    // Failure returns the original owner before any host work can be accepted.
    fn admit(
        &self,
        command: Command,
        owner: Owner,
        transfer: bool,
    ) -> Result<(), (Command, Owner, &'static str)> {
        self.start_worker();
        let mut state = self.0.admission.lock();
        if state.closed {
            return Err((command, owner, "Metal event loop closed"));
        }
        if state.pending.len() >= CAPACITY || (state.cpu_access && !transfer) {
            return Err((command, owner, "SGFX busy"));
        }
        let id = match self.id() {
            Ok(id) => id,
            Err(e) => return Err((command, owner, e)),
        };
        state.pending.insert(id, owner);
        if let Err(error) = self.0.tx.send(Request { id, command }) {
            return Err((
                error.0.command,
                state.pending.remove(&id).unwrap(),
                "Metal event loop closed",
            ));
        }
        drop(state);
        (self.0.wake)();
        Ok(())
    }
    fn admit_wait(
        &self,
        mut command: Command,
        mut owner: Owner,
        transfer: bool,
    ) -> Result<(), &'static str> {
        let task = scarlet::task::mytask().ok_or("SGFX requires a scheduled task")?;
        loop {
            match self.admit(command, owner, transfer) {
                Ok(()) => return Ok(()),
                Err((c, o, "SGFX busy")) => {
                    command = c;
                    owner = o;
                }
                Err((_, _, e)) => return Err(e),
            }
            self.0.changed.clone().wait_result_until_owned(
                task.get_id(),
                task.get_trapframe(),
                || {
                    let state = self.0.admission.lock();
                    state.closed
                        || (state.pending.len() < CAPACITY && (!state.cpu_access || transfer))
                },
            );
        }
    }
    pub(super) fn call(&self, command: Command, transfer: bool) -> ResultBytes {
        let ticket = Ticket::new();
        self.admit_wait(command, Owner::Call(ticket.clone()), transfer)?;
        ticket.take()
    }
    pub(super) fn enqueue(
        &self,
        command: Command,
        submission: GpuSubmission,
    ) -> Result<(), GpuBackendEnqueueError> {
        self.admit(command, Owner::Submission(submission), false)
            .map_err(|(_, owner, e)| {
                let Owner::Submission(s) = owner else {
                    unreachable!()
                };
                if e == "SGFX busy" {
                    GpuBackendEnqueueError::Busy(s)
                } else {
                    GpuBackendEnqueueError::Rejected(GpuBackendSubmitError::Unavailable(e), s)
                }
            })
    }
    pub(super) fn release(&self, command: Command) {
        if self.0.tx.send(Request { id: 0, command }).is_ok() {
            (self.0.wake)();
        }
    }
    pub fn begin_present(
        &self,
        resource: GpuDisplayResource,
        region: DisplayRegion,
        owners: PresentationOwners,
    ) -> Result<Option<Box<dyn PresentationWait>>, &'static str> {
        if !self.0.presentation || resource.backend_cookie() != self.cookie() {
            return Err("Foreign SGFX display resource");
        }
        let image = self.image(resource.resource_id().into())?;
        if image.display_resource().is_none() {
            return Err("SGFX image is not presentable");
        }
        let rect = Rect {
            x: region.x,
            y: region.y,
            width: region.width,
            height: region.height,
        };
        let ticket = Ticket::new();
        self.admit(
            Command::Present(image.info.command_resource_token, rect),
            Owner::Present(ticket.clone(), owners),
            false,
        )
        .map_err(|(_, _, e)| e)?;
        Ok(Some(Box::new(PresentTicket(ticket))))
    }
    pub(super) fn reserve_cpu(&self) -> Result<(), &'static str> {
        let task = scarlet::task::mytask().ok_or("CPU access requires a task")?;
        // Close admission before draining accepted writes. A continuous stream
        // on another context must not keep a CPU transfer waiting indefinitely.
        let mut reserved = false;
        loop {
            {
                let mut state = self.0.admission.lock();
                if state.closed {
                    return Err("Metal event loop closed");
                }
                if !reserved && !state.cpu_access {
                    state.cpu_access = true;
                    reserved = true;
                }
                if reserved && state.pending.is_empty() {
                    return Ok(());
                }
            }
            self.0.changed.clone().wait_result_until_owned(
                task.get_id(),
                task.get_trapframe(),
                || {
                    let state = self.0.admission.lock();
                    state.closed
                        || if reserved {
                            state.pending.is_empty()
                        } else {
                            !state.cpu_access
                        }
                },
            );
        }
    }
    pub(super) fn release_cpu(&self) {
        self.0.admission.lock().cpu_access = false;
        self.0.changed.wake_all();
    }
}
