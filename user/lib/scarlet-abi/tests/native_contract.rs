//! Representation baselines for the native userspace ABI.
//!
//! Literal sizes, offsets, and numbers deliberately do not derive their expected
//! values from the implementation. Updating this baseline requires ABI review;
//! these tests alone do not establish syscall semantics or hardware support.

use core::mem::{align_of, offset_of, size_of};
use scarlet_abi as abi;

macro_rules! assert_layout {
    ($ty:ty, $size:literal, $align:literal, { $($field:ident: $offset:literal),+ $(,)? }) => {
        assert_eq!(size_of::<$ty>(), $size, stringify!($ty));
        assert_eq!(align_of::<$ty>(), $align, stringify!($ty));
        $(assert_eq!(offset_of!($ty, $field), $offset, stringify!($field));)+
    };
}

#[test]
fn native_identifiers_and_file_metadata_layout() {
    assert_eq!(size_of::<abi::RawHandle>(), 4);
    assert_eq!(size_of::<abi::Pid>(), 4);
    assert_eq!(size_of::<abi::Tid>(), 4);
    assert_layout!(abi::RawFileMetadata, 56, 8, {
        size: 0, file_type: 8, permissions: 12, created: 16, modified: 24,
        accessed: 32, file_id: 40, link_count: 48, _reserved: 52,
    });
}

#[test]
fn scheduler_v1_field_layouts() {
    assert_layout!(abi::RawTaskDeadlineParams, 24, 8, {
        runtime_ns: 0, deadline_ns: 8, period_ns: 16,
    });
    assert_layout!(abi::RawSchedulerAttrV1, 128, 8, {
        size: 0, version: 4, policy: 8, flags: 12, affinity_kind: 16,
        cpu_id: 20, nice: 24, util_min: 28, cpu_mask_ptr: 32,
        cpu_mask_bytes: 40, cpu_mask_nbits: 44, runtime_ns: 48,
        deadline_ns: 56, period_ns: 64, deadline_cpu_id: 72,
        reserved0: 76, reserved: 80,
    });
    assert_layout!(abi::RawSchedulerStateV1, 160, 8, {
        size: 0, version: 4, status: 8, policy: 12, flags: 16,
        affinity_kind: 20, configured_cpu_id: 24, current_cpu_id: 28,
        queued_cpu_id: 32, nice: 36, util_min: 40, reserved0: 44,
        fair_vruntime_ns: 48, fair_vdeadline_ns: 56, fair_slice_remaining_ns: 64,
        deadline_runtime_remaining_ns: 72, deadline_absolute_ns: 80,
        deadline_replenishment_ns: 88, deadline_admission_units: 96,
        reserved1: 100, deadline_miss_count: 104, deadline_overrun_count: 112,
        reserved: 120,
    });
}

#[test]
fn debug_snapshot_v1_field_layouts() {
    assert_eq!(abi::TASK_DEBUG_INFO_VERSION_V1, 1);
    assert_eq!(abi::CPU_DEBUG_INFO_VERSION_V1, 1);
    assert_layout!(abi::RawTaskDebugInfoV1, 64, 8, {
        size: 0, version: 4, state: 6, task_type: 7, flags: 8, cpu_id: 12,
        pid: 16, tgid: 24, observed_pc: 32, syscall_number: 40,
        syscall_pc: 48, cpu_time_ns: 56,
    });
    assert_layout!(abi::RawCpuDebugInfoV1, 64, 8, {
        size: 0, version: 4, flags: 6, cpu_id: 8, reserved: 12,
        current_task_id: 16, timer_irq_count: 24, breadcrumb_phase: 32,
        breadcrumb_aux: 40, breadcrumb_aux2: 48, timer_deadline_ns: 56,
    });
}

#[test]
fn scheduler_v1_headers_and_reserved_fields() {
    assert_eq!(abi::SCHEDULER_CONTROL_VERSION_V1, 1);
    assert_eq!(abi::RAW_SCHEDULER_ATTR_V1_SIZE, 128);
    assert_eq!(abi::RAW_SCHEDULER_STATE_V1_SIZE, 160);

    let attr = abi::RawSchedulerAttrV1::new();
    assert_eq!(attr, abi::RawSchedulerAttrV1::default());
    assert_eq!(
        (attr.size, attr.version, attr.policy, attr.flags),
        (128, 1, 0, 0)
    );
    assert_eq!(
        (attr.affinity_kind, attr.cpu_id, attr.deadline_cpu_id),
        (0, u32::MAX, u32::MAX)
    );
    assert_eq!(
        (attr.cpu_mask_ptr, attr.cpu_mask_bytes, attr.cpu_mask_nbits),
        (0, 0, 0)
    );
    assert_eq!(attr.reserved0, 0);
    assert_eq!(attr.reserved, [0; 6]);

    let state = abi::RawSchedulerStateV1::new();
    assert_eq!(state, abi::RawSchedulerStateV1::default());
    assert_eq!(
        (state.size, state.version, state.status, state.flags),
        (160, 1, 0, 0)
    );
    assert_eq!(
        (
            state.configured_cpu_id,
            state.current_cpu_id,
            state.queued_cpu_id
        ),
        (u32::MAX, u32::MAX, u32::MAX)
    );
    assert_eq!((state.reserved0, state.reserved1), (0, 0));
    assert_eq!(state.reserved, [0; 5]);
}

#[test]
fn scheduler_results_preserve_unknown_values() {
    let results = [
        abi::RawSchedulerResult::Ok,
        abi::RawSchedulerResult::BadAddress,
        abi::RawSchedulerResult::BadSize,
        abi::RawSchedulerResult::UnsupportedVersion,
        abi::RawSchedulerResult::InvalidFlags,
        abi::RawSchedulerResult::InvalidPolicy,
        abi::RawSchedulerResult::InvalidArgument,
        abi::RawSchedulerResult::CpuOffline,
        abi::RawSchedulerResult::EmptyCpuMask,
        abi::RawSchedulerResult::AdmissionFailed,
        abi::RawSchedulerResult::Busy,
        abi::RawSchedulerResult::BufferTooSmall,
    ];
    for (raw, expected) in results.into_iter().enumerate() {
        assert_eq!(expected.as_raw(), raw as u32);
        assert_eq!(
            abi::RawSchedulerResult::from_raw(raw as u32),
            Some(expected)
        );
    }
    for raw in [12, u32::MAX] {
        assert_eq!(abi::RawSchedulerResult::from_raw(raw), None);
    }
    let statuses = [
        abi::RawSchedulerStatus::Unknown,
        abi::RawSchedulerStatus::Running,
        abi::RawSchedulerStatus::Queued,
        abi::RawSchedulerStatus::Blocked,
        abi::RawSchedulerStatus::Throttled,
    ];
    for (raw, expected) in statuses.into_iter().enumerate() {
        assert_eq!(expected.as_raw(), raw as u32);
        assert_eq!(
            abi::RawSchedulerStatus::from_raw(raw as u32),
            Some(expected)
        );
    }
    for raw in [5, u32::MAX] {
        let mut state = abi::RawSchedulerStateV1::new();
        state.status = raw;
        assert_eq!(state.scheduler_status(), None);
        assert_eq!(state.status, raw);
    }
}

#[test]
fn native_syscall_numbers() {
    // The exhaustive match also makes additions require a baseline review.
    macro_rules! check_numbers {
        ($($name:ident = $number:literal),+ $(,)?) => {{
            fn expected(syscall: abi::Syscall) -> usize {
                match syscall { $(abi::Syscall::$name => $number,)+ }
            }
            for syscall in [$(abi::Syscall::$name,)+] {
                assert_eq!(syscall as usize, expected(syscall), "{syscall:?}");
            }
        }};
    }
    check_numbers! {
        Invalid = 0, Exit = 1, Clone = 2, Execve = 3, ExecveABI = 4,
        Waitpid = 5, Kill = 6, Getpid = 7, Getppid = 8, Brk = 12, Sbrk = 13,
        Putchar = 16, Getchar = 17, Sleep = 20, Yield = 21, GetRandom = 22,
        ExitGroup = 23, GetTaskInfoCount = 24, GetTaskInfoList = 25,
        CreateSession = 26, GetSessionId = 27, GetProcessGroupId = 28,
        SetProcessGroup = 29, SetTls = 30, GetTls = 31, SetTidAddress = 32,
        ThreadDetach = 33, ThreadExitCleanup = 34, MonotonicTime = 35,
        GetCpuUsageInfo = 36, SystemTime = 37, SetTaskUtilMin = 38,
        GetTaskUtilMin = 39, SetTaskNice = 40, GetTaskNice = 41,
        SetTaskCpuAffinity = 42, GetTaskCpuAffinity = 43,
        SetTaskDeadline = 44, GetTaskDeadline = 45, SetSchedulerAttr = 46,
        GetSchedulerAttr = 47, GetSchedulerState = 48, FutexWait = 49,
        FutexWake = 50, RegisterAbiZone = 90, UnregisterAbiZone = 91,
        CreateNamespace = 92, HandleQuery = 100, HandleSetRole = 101,
        HandleClose = 102, HandleDuplicate = 103, HandleControl = 110,
        StreamRead = 200, StreamWrite = 201, Poll = 202, FileSeek = 300,
        FileTruncate = 301, FileMetadata = 302, VfsOpen = 400, VfsRemove = 401,
        VfsCreateFile = 402, VfsCreateDirectory = 403, VfsChangeDirectory = 404,
        VfsTruncate = 405, VfsCreateSymlink = 406, VfsReadlink = 407,
        VfsGetCwdPath = 408, VfsRename = 409, VfsMetadata = 410,
        VfsCreateHardlink = 411, FsMount = 500, FsUmount = 501,
        FsPivotRoot = 502, Pipe = 600, EventSendDirect = 615,
        EventSendGroup = 616, SharedMemoryCreate = 620, SharedMemoryResize = 621,
        SocketSendHandle = 630, SocketRecvHandle = 631,
        SocketSendHandleAndData = 632, SocketRecvHandleAndData = 633,
        EventHandlerRegister = 640, EventHandlerUnregister = 641,
        EventMask = 642, EventReturn = 643, EventHandlerRegisterWithRestorer = 644,
        MemoryMap = 700, MemoryUnmap = 701, SocketCreate = 900, SocketBind = 901,
        SocketListen = 902, SocketConnect = 903, SocketAccept = 904,
        Socketpair = 905, SocketShutdown = 906, SocketRecvFrom = 907,
        SocketSendTo = 908, SocketBindInterface = 909, NetworkSetIpv4 = 910,
        NetworkSetGateway = 911, NetworkSetNetmask = 913,
        NetworkListInterfaces = 914, NetworkConfigureIpv4 = 915,
        NetworkListInterfacesV2 = 916, NetworkClearIpv4 = 917,
        SocketGetLocalAddress = 918, SocketGetPeerAddress = 919,
        GetCpuDebugInfo = 997, GetTaskDebugInfo = 998, ProfilerDump = 999,
        Shutdown = 1000, ShvVmCreate = 1100, ShvVcpuCreate = 1101,
        ShvVcpuRun = 1102, LsmLoad = 1200, LsmUnload = 1201, LsmList = 1202,
    }
}
