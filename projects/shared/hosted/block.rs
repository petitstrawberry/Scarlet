//! Root image adapter for Scarlet's block-device interface.

use crate::root_image::{RootImage, SECTOR_SIZE};
use scarlet::{
    arch::Trapframe,
    device::{
        Device, DeviceType,
        block::{
            BlockDevice,
            request::{BlockIORequest, BlockIORequestType, BlockIOResult},
        },
    },
    interrupt::{
        CpuId, InterruptClaim, InterruptId, InterruptManager, InterruptResult, InterruptSource,
    },
    object::capability::{
        ControlOps, MemoryMappingInfo, MemoryMappingOps, ReadyInterest, SelectWaitOutcome,
        Selectable,
    },
    sync::{IrqSpinLock, waker::Waker},
};
use std::{
    any::Any,
    collections::VecDeque,
    io,
    sync::{Arc, Mutex, mpsc},
};

pub struct RootBlock {
    size: usize,
    submissions: mpsc::Sender<Submission>,
    completions: Option<Arc<CompletionSource>>,
    pending: IrqSpinLock<Vec<Box<BlockIORequest>>>,
}

struct Submission {
    requests: Vec<Box<BlockIORequest>>,
    reply: Reply,
}

enum Reply {
    Boot(mpsc::SyncSender<Vec<BlockIOResult>>),
    Task(Arc<TaskCompletion>),
}

struct TaskCompletion {
    waker: Arc<Waker>,
    results: IrqSpinLock<Option<Vec<BlockIOResult>>>,
}

struct CompletionSource {
    cpu: CpuId,
    line: InterruptId,
    inbox: Mutex<VecDeque<(Arc<TaskCompletion>, Vec<BlockIOResult>)>>,
}

impl RootBlock {
    /// Start the host service for synchronous boot probes.
    pub fn new(image: RootImage) -> io::Result<Self> {
        Self::start(image, None)
    }

    /// Attach a root device to the platform IRQ controller before scheduling.
    pub fn attach(image: RootImage, cpu: CpuId, line: InterruptId) -> io::Result<Self> {
        let source = Arc::new(CompletionSource {
            cpu,
            line,
            inbox: Mutex::new(VecDeque::new()),
        });
        let device = Self::start(image, Some(source.clone()))?;
        let controller = InterruptManager::global();
        controller
            .register_interrupt_source(line, source)
            .map_err(|error| {
                io::Error::other(format!("Cannot register root image IRQ: {error}"))
            })?;
        controller
            .enable_external_interrupt(line, cpu)
            .map_err(|error| io::Error::other(format!("Cannot enable root image IRQ: {error}")))?;
        Ok(device)
    }

    fn start(image: RootImage, completions: Option<Arc<CompletionSource>>) -> io::Result<Self> {
        let size = image.size();
        let (submissions, receive) = mpsc::channel::<Submission>();
        let host_completions = completions.clone();
        std::thread::Builder::new()
            .name("scarlet-root-image".into())
            .spawn(move || {
                while let Ok(submission) = receive.recv() {
                    let results = execute_batch(&image, submission.requests);
                    match submission.reply {
                        Reply::Boot(reply) => {
                            let _ = reply.send(results);
                        }
                        Reply::Task(task) => {
                            let source = host_completions
                                .as_ref()
                                .expect("Task reply has an interrupt route");
                            source.inbox.lock().unwrap().push_back((task, results));
                            source.signal();
                        }
                    }
                }
            })?;
        Ok(Self {
            size,
            submissions,
            completions,
            pending: IrqSpinLock::new(Vec::new()),
        })
    }

    fn submit(&self, submission: Submission) -> Result<(), Vec<BlockIOResult>> {
        self.submissions.send(submission).map_err(|error| {
            error
                .0
                .requests
                .into_iter()
                .map(|request| BlockIOResult {
                    request,
                    result: Err("Root image service has stopped"),
                })
                .collect()
        })
    }
}

impl CompletionSource {
    fn signal(&self) {
        scarlet::hosted::raise_external_interrupt(self.cpu, self.line)
            .expect("Registered root image interrupt must remain available");
    }
}

impl InterruptSource for CompletionSource {
    fn interrupt_id(&self) -> Option<InterruptId> {
        Some(self.line)
    }

    fn claim_interrupt(&self) -> InterruptResult<InterruptClaim> {
        let (completion, more) = {
            let mut inbox = self.inbox.lock().unwrap();
            (inbox.pop_front(), !inbox.is_empty())
        };
        if more {
            self.signal();
        }
        let Some((task, results)) = completion else {
            return Ok(InterruptClaim::NotMine);
        };
        *task.results.lock() = Some(results);
        task.waker.wake_all();
        Ok(InterruptClaim::Handled)
    }
}

impl Device for RootBlock {
    fn name(&self) -> &'static str {
        "hostroot"
    }

    fn device_type(&self) -> DeviceType {
        DeviceType::Block
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn as_block_device(&self) -> Option<&dyn BlockDevice> {
        Some(self)
    }

    fn into_block_device(self: Arc<Self>) -> Option<Arc<dyn BlockDevice>> {
        Some(self)
    }
}

impl ControlOps for RootBlock {}

impl MemoryMappingOps for RootBlock {
    fn get_mapping_info(&self, _: usize, _: usize) -> Result<MemoryMappingInfo, &'static str> {
        Err("Root block requests use positional I/O")
    }

    fn supports_mmap(&self) -> bool {
        false
    }
}

impl Selectable for RootBlock {
    fn wait_until_ready(
        &self,
        _: ReadyInterest,
        _: &mut Trapframe,
        _: Option<u64>,
        _: u64,
    ) -> SelectWaitOutcome {
        SelectWaitOutcome::Ready
    }
}

impl BlockDevice for RootBlock {
    fn get_disk_name(&self) -> &'static str {
        self.name()
    }

    fn get_disk_size(&self) -> usize {
        self.size
    }

    fn get_sector_size(&self) -> usize {
        SECTOR_SIZE
    }

    fn enqueue_request(&self, request: Box<BlockIORequest>) {
        self.pending.lock().push(request);
    }

    fn process_requests(&self) -> Vec<BlockIOResult> {
        let pending = std::mem::take(&mut *self.pending.lock());
        self.submit_requests(pending)
    }

    fn submit_requests(&self, requests: Vec<Box<BlockIORequest>>) -> Vec<BlockIOResult> {
        if requests.is_empty() {
            return Vec::new();
        }
        if self.completions.is_some()
            && let Some(cpu) = scarlet::arch::try_get_cpuid()
            && scarlet::sched::scheduler::scheduler_ready(cpu)
            && let Some(task) = scarlet::task::mytask()
        {
            assert!(
                scarlet::sync::preemptible(),
                "Scheduled root I/O needs a sleepable context"
            );
            let completion = Arc::new(TaskCompletion {
                waker: Arc::new(Waker::new_uninterruptible("root-image")),
                results: IrqSpinLock::new(None),
            });
            if let Err(results) = self.submit(Submission {
                requests,
                reply: Reply::Task(completion.clone()),
            }) {
                return results;
            }
            loop {
                if let Some(results) = completion.results.lock().take() {
                    return results;
                }
                completion.waker.clone().wait_result_until_owned(
                    task.get_id(),
                    task.get_trapframe(),
                    || completion.results.lock().is_some(),
                );
            }
        }

        let (reply, receive) = mpsc::sync_channel(1);
        if let Err(results) = self.submit(Submission {
            requests,
            reply: Reply::Boot(reply),
        }) {
            return results;
        }
        receive
            .recv()
            .expect("Host root image service must respond to a boot request")
    }
}

fn execute_batch(image: &RootImage, requests: Vec<Box<BlockIORequest>>) -> Vec<BlockIOResult> {
    let mut wrote_data = false;
    let mut results: Vec<_> = requests
        .into_iter()
        .map(|mut request| {
            let result =
                if request.sector_count.checked_mul(SECTOR_SIZE) != Some(request.buffer.len()) {
                    Err("Invalid root-image request length")
                } else {
                    match request.request_type {
                        BlockIORequestType::Read => {
                            image.read_sectors(request.sector, &mut request.buffer)
                        }
                        BlockIORequestType::Write => {
                            wrote_data = true;
                            image.write_sectors_unsynced(request.sector, &request.buffer)
                        }
                    }
                    .map_err(|_| "Root image I/O failed")
                };
            BlockIOResult { request, result }
        })
        .collect();
    if wrote_data && image.sync().is_err() {
        for result in &mut results {
            if result.request.request_type == BlockIORequestType::Write && result.result.is_ok() {
                result.result = Err("Cannot synchronize root image writes");
            }
        }
    }
    results
}
