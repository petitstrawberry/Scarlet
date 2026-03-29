// Generated from /tmp/arm64_ntos_26100.txt
// Regenerate: cargo run --release --manifest-path tools/ntsyscall_gen/Cargo.toml -- --arch aarch64 --text /tmp/arm64_ntos_26100.txt
//
// This file is auto-generated. DO NOT EDIT MANUALLY.

//! NT syscall table extracted from ntdll.dll
//! Contains syscall numbers for ARM64 Windows binaries.

/// Source ntdll.dll version info
pub const NTDLL_VERSION: &str = "Windows 11 build 26100 (from hfiref0x/SyscallTables)";

/// A single NT syscall entry
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NtSyscallEntry {
    /// Syscall number (from SVC immediate)
    pub number: u16,
    /// Function name (e.g., "NtCreateFile")
    pub name: &'static str,
}

/// Complete syscall table sorted by syscall number
pub const NT_SYSCALL_TABLE: &[NtSyscallEntry] = &[
    NtSyscallEntry {
        number: 0x0000,
        name: "NtAccessCheck",
    },
    NtSyscallEntry {
        number: 0x0001,
        name: "NtWorkerFactoryWorkerReady",
    },
    NtSyscallEntry {
        number: 0x0002,
        name: "NtAcceptConnectPort",
    },
    NtSyscallEntry {
        number: 0x0003,
        name: "NtMapUserPhysicalPagesScatter",
    },
    NtSyscallEntry {
        number: 0x0004,
        name: "NtWaitForSingleObject",
    },
    NtSyscallEntry {
        number: 0x0005,
        name: "NtCallbackReturn",
    },
    NtSyscallEntry {
        number: 0x0006,
        name: "NtReadFile",
    },
    NtSyscallEntry {
        number: 0x0007,
        name: "NtDeviceIoControlFile",
    },
    NtSyscallEntry {
        number: 0x0008,
        name: "NtWriteFile",
    },
    NtSyscallEntry {
        number: 0x0009,
        name: "NtRemoveIoCompletion",
    },
    NtSyscallEntry {
        number: 0x000A,
        name: "NtReleaseSemaphore",
    },
    NtSyscallEntry {
        number: 0x000B,
        name: "NtReplyWaitReceivePort",
    },
    NtSyscallEntry {
        number: 0x000C,
        name: "NtReplyPort",
    },
    NtSyscallEntry {
        number: 0x000D,
        name: "NtSetInformationThread",
    },
    NtSyscallEntry {
        number: 0x000E,
        name: "NtSetEvent",
    },
    NtSyscallEntry {
        number: 0x000F,
        name: "NtClose",
    },
    NtSyscallEntry {
        number: 0x0010,
        name: "NtQueryObject",
    },
    NtSyscallEntry {
        number: 0x0011,
        name: "NtQueryInformationFile",
    },
    NtSyscallEntry {
        number: 0x0012,
        name: "NtOpenKey",
    },
    NtSyscallEntry {
        number: 0x0013,
        name: "NtEnumerateValueKey",
    },
    NtSyscallEntry {
        number: 0x0014,
        name: "NtFindAtom",
    },
    NtSyscallEntry {
        number: 0x0015,
        name: "NtQueryDefaultLocale",
    },
    NtSyscallEntry {
        number: 0x0016,
        name: "NtQueryKey",
    },
    NtSyscallEntry {
        number: 0x0017,
        name: "NtQueryValueKey",
    },
    NtSyscallEntry {
        number: 0x0018,
        name: "NtAllocateVirtualMemory",
    },
    NtSyscallEntry {
        number: 0x0019,
        name: "NtQueryInformationProcess",
    },
    NtSyscallEntry {
        number: 0x001A,
        name: "NtWaitForMultipleObjects32",
    },
    NtSyscallEntry {
        number: 0x001B,
        name: "NtWriteFileGather",
    },
    NtSyscallEntry {
        number: 0x001C,
        name: "NtSetInformationProcess",
    },
    NtSyscallEntry {
        number: 0x001D,
        name: "NtCreateKey",
    },
    NtSyscallEntry {
        number: 0x001E,
        name: "NtFreeVirtualMemory",
    },
    NtSyscallEntry {
        number: 0x001F,
        name: "NtImpersonateClientOfPort",
    },
    NtSyscallEntry {
        number: 0x0020,
        name: "NtReleaseMutant",
    },
    NtSyscallEntry {
        number: 0x0021,
        name: "NtQueryInformationToken",
    },
    NtSyscallEntry {
        number: 0x0022,
        name: "NtRequestWaitReplyPort",
    },
    NtSyscallEntry {
        number: 0x0023,
        name: "NtQueryVirtualMemory",
    },
    NtSyscallEntry {
        number: 0x0024,
        name: "NtOpenThreadToken",
    },
    NtSyscallEntry {
        number: 0x0025,
        name: "NtQueryInformationThread",
    },
    NtSyscallEntry {
        number: 0x0026,
        name: "NtOpenProcess",
    },
    NtSyscallEntry {
        number: 0x0027,
        name: "NtSetInformationFile",
    },
    NtSyscallEntry {
        number: 0x0028,
        name: "NtMapViewOfSection",
    },
    NtSyscallEntry {
        number: 0x0029,
        name: "NtAccessCheckAndAuditAlarm",
    },
    NtSyscallEntry {
        number: 0x002A,
        name: "NtUnmapViewOfSection",
    },
    NtSyscallEntry {
        number: 0x002B,
        name: "NtReplyWaitReceivePortEx",
    },
    NtSyscallEntry {
        number: 0x002C,
        name: "NtTerminateProcess",
    },
    NtSyscallEntry {
        number: 0x002D,
        name: "NtSetEventBoostPriority",
    },
    NtSyscallEntry {
        number: 0x002E,
        name: "NtReadFileScatter",
    },
    NtSyscallEntry {
        number: 0x002F,
        name: "NtOpenThreadTokenEx",
    },
    NtSyscallEntry {
        number: 0x0030,
        name: "NtOpenProcessTokenEx",
    },
    NtSyscallEntry {
        number: 0x0031,
        name: "NtQueryPerformanceCounter",
    },
    NtSyscallEntry {
        number: 0x0032,
        name: "NtEnumerateKey",
    },
    NtSyscallEntry {
        number: 0x0033,
        name: "NtOpenFile",
    },
    NtSyscallEntry {
        number: 0x0034,
        name: "NtDelayExecution",
    },
    NtSyscallEntry {
        number: 0x0035,
        name: "NtQueryDirectoryFile",
    },
    NtSyscallEntry {
        number: 0x0036,
        name: "NtQuerySystemInformation",
    },
    NtSyscallEntry {
        number: 0x0037,
        name: "NtOpenSection",
    },
    NtSyscallEntry {
        number: 0x0038,
        name: "NtQueryTimer",
    },
    NtSyscallEntry {
        number: 0x0039,
        name: "NtFsControlFile",
    },
    NtSyscallEntry {
        number: 0x003A,
        name: "NtWriteVirtualMemory",
    },
    NtSyscallEntry {
        number: 0x003B,
        name: "NtCloseObjectAuditAlarm",
    },
    NtSyscallEntry {
        number: 0x003C,
        name: "NtDuplicateObject",
    },
    NtSyscallEntry {
        number: 0x003D,
        name: "NtQueryAttributesFile",
    },
    NtSyscallEntry {
        number: 0x003E,
        name: "NtClearEvent",
    },
    NtSyscallEntry {
        number: 0x003F,
        name: "NtReadVirtualMemory",
    },
    NtSyscallEntry {
        number: 0x0040,
        name: "NtOpenEvent",
    },
    NtSyscallEntry {
        number: 0x0041,
        name: "NtAdjustPrivilegesToken",
    },
    NtSyscallEntry {
        number: 0x0042,
        name: "NtDuplicateToken",
    },
    NtSyscallEntry {
        number: 0x0043,
        name: "NtContinue",
    },
    NtSyscallEntry {
        number: 0x0044,
        name: "NtQueryDefaultUILanguage",
    },
    NtSyscallEntry {
        number: 0x0045,
        name: "NtQueueApcThread",
    },
    NtSyscallEntry {
        number: 0x0046,
        name: "NtYieldExecution",
    },
    NtSyscallEntry {
        number: 0x0047,
        name: "NtAddAtom",
    },
    NtSyscallEntry {
        number: 0x0048,
        name: "NtCreateEvent",
    },
    NtSyscallEntry {
        number: 0x0049,
        name: "NtQueryVolumeInformationFile",
    },
    NtSyscallEntry {
        number: 0x004A,
        name: "NtCreateSection",
    },
    NtSyscallEntry {
        number: 0x004B,
        name: "NtFlushBuffersFile",
    },
    NtSyscallEntry {
        number: 0x004C,
        name: "NtApphelpCacheControl",
    },
    NtSyscallEntry {
        number: 0x004D,
        name: "NtCreateProcessEx",
    },
    NtSyscallEntry {
        number: 0x004E,
        name: "NtCreateThread",
    },
    NtSyscallEntry {
        number: 0x004F,
        name: "NtIsProcessInJob",
    },
    NtSyscallEntry {
        number: 0x0050,
        name: "NtProtectVirtualMemory",
    },
    NtSyscallEntry {
        number: 0x0051,
        name: "NtQuerySection",
    },
    NtSyscallEntry {
        number: 0x0052,
        name: "NtResumeThread",
    },
    NtSyscallEntry {
        number: 0x0053,
        name: "NtTerminateThread",
    },
    NtSyscallEntry {
        number: 0x0054,
        name: "NtReadRequestData",
    },
    NtSyscallEntry {
        number: 0x0055,
        name: "NtCreateFile",
    },
    NtSyscallEntry {
        number: 0x0056,
        name: "NtQueryEvent",
    },
    NtSyscallEntry {
        number: 0x0057,
        name: "NtWriteRequestData",
    },
    NtSyscallEntry {
        number: 0x0058,
        name: "NtOpenDirectoryObject",
    },
    NtSyscallEntry {
        number: 0x0059,
        name: "NtAccessCheckByTypeAndAuditAlarm",
    },
    NtSyscallEntry {
        number: 0x005A,
        name: "NtQuerySystemTime",
    },
    NtSyscallEntry {
        number: 0x005B,
        name: "NtWaitForMultipleObjects",
    },
    NtSyscallEntry {
        number: 0x005C,
        name: "NtSetInformationObject",
    },
    NtSyscallEntry {
        number: 0x005D,
        name: "NtCancelIoFile",
    },
    NtSyscallEntry {
        number: 0x005E,
        name: "NtTraceEvent",
    },
    NtSyscallEntry {
        number: 0x005F,
        name: "NtPowerInformation",
    },
    NtSyscallEntry {
        number: 0x0060,
        name: "NtSetValueKey",
    },
    NtSyscallEntry {
        number: 0x0061,
        name: "NtCancelTimer",
    },
    NtSyscallEntry {
        number: 0x0062,
        name: "NtSetTimer",
    },
    NtSyscallEntry {
        number: 0x0063,
        name: "NtAccessCheckByType",
    },
    NtSyscallEntry {
        number: 0x0064,
        name: "NtAccessCheckByTypeResultList",
    },
    NtSyscallEntry {
        number: 0x0065,
        name: "NtAccessCheckByTypeResultListAndAuditAlarm",
    },
    NtSyscallEntry {
        number: 0x0066,
        name: "NtAccessCheckByTypeResultListAndAuditAlarmByHandle",
    },
    NtSyscallEntry {
        number: 0x0067,
        name: "NtAcquireCrossVmMutant",
    },
    NtSyscallEntry {
        number: 0x0068,
        name: "NtAcquireProcessActivityReference",
    },
    NtSyscallEntry {
        number: 0x0069,
        name: "NtAddAtomEx",
    },
    NtSyscallEntry {
        number: 0x006A,
        name: "NtAddBootEntry",
    },
    NtSyscallEntry {
        number: 0x006B,
        name: "NtAddDriverEntry",
    },
    NtSyscallEntry {
        number: 0x006C,
        name: "NtAdjustGroupsToken",
    },
    NtSyscallEntry {
        number: 0x006D,
        name: "NtAdjustTokenClaimsAndDeviceGroups",
    },
    NtSyscallEntry {
        number: 0x006E,
        name: "NtAlertMultipleThreadByThreadId",
    },
    NtSyscallEntry {
        number: 0x006F,
        name: "NtAlertResumeThread",
    },
    NtSyscallEntry {
        number: 0x0070,
        name: "NtAlertThread",
    },
    NtSyscallEntry {
        number: 0x0071,
        name: "NtAlertThreadByThreadId",
    },
    NtSyscallEntry {
        number: 0x0072,
        name: "NtAlertThreadByThreadIdEx",
    },
    NtSyscallEntry {
        number: 0x0073,
        name: "NtAllocateLocallyUniqueId",
    },
    NtSyscallEntry {
        number: 0x0074,
        name: "NtAllocateReserveObject",
    },
    NtSyscallEntry {
        number: 0x0075,
        name: "NtAllocateUserPhysicalPages",
    },
    NtSyscallEntry {
        number: 0x0076,
        name: "NtAllocateUserPhysicalPagesEx",
    },
    NtSyscallEntry {
        number: 0x0077,
        name: "NtAllocateUuids",
    },
    NtSyscallEntry {
        number: 0x0078,
        name: "NtAllocateVirtualMemoryEx",
    },
    NtSyscallEntry {
        number: 0x0079,
        name: "NtAlpcAcceptConnectPort",
    },
    NtSyscallEntry {
        number: 0x007A,
        name: "NtAlpcCancelMessage",
    },
    NtSyscallEntry {
        number: 0x007B,
        name: "NtAlpcConnectPort",
    },
    NtSyscallEntry {
        number: 0x007C,
        name: "NtAlpcConnectPortEx",
    },
    NtSyscallEntry {
        number: 0x007D,
        name: "NtAlpcCreatePort",
    },
    NtSyscallEntry {
        number: 0x007E,
        name: "NtAlpcCreatePortSection",
    },
    NtSyscallEntry {
        number: 0x007F,
        name: "NtAlpcCreateResourceReserve",
    },
    NtSyscallEntry {
        number: 0x0080,
        name: "NtAlpcCreateSectionView",
    },
    NtSyscallEntry {
        number: 0x0081,
        name: "NtAlpcCreateSecurityContext",
    },
    NtSyscallEntry {
        number: 0x0082,
        name: "NtAlpcDeletePortSection",
    },
    NtSyscallEntry {
        number: 0x0083,
        name: "NtAlpcDeleteResourceReserve",
    },
    NtSyscallEntry {
        number: 0x0084,
        name: "NtAlpcDeleteSectionView",
    },
    NtSyscallEntry {
        number: 0x0085,
        name: "NtAlpcDeleteSecurityContext",
    },
    NtSyscallEntry {
        number: 0x0086,
        name: "NtAlpcDisconnectPort",
    },
    NtSyscallEntry {
        number: 0x0087,
        name: "NtAlpcImpersonateClientContainerOfPort",
    },
    NtSyscallEntry {
        number: 0x0088,
        name: "NtAlpcImpersonateClientOfPort",
    },
    NtSyscallEntry {
        number: 0x0089,
        name: "NtAlpcOpenSenderProcess",
    },
    NtSyscallEntry {
        number: 0x008A,
        name: "NtAlpcOpenSenderThread",
    },
    NtSyscallEntry {
        number: 0x008B,
        name: "NtAlpcQueryInformation",
    },
    NtSyscallEntry {
        number: 0x008C,
        name: "NtAlpcQueryInformationMessage",
    },
    NtSyscallEntry {
        number: 0x008D,
        name: "NtAlpcRevokeSecurityContext",
    },
    NtSyscallEntry {
        number: 0x008E,
        name: "NtAlpcSendWaitReceivePort",
    },
    NtSyscallEntry {
        number: 0x008F,
        name: "NtAlpcSetInformation",
    },
    NtSyscallEntry {
        number: 0x0090,
        name: "NtAreMappedFilesTheSame",
    },
    NtSyscallEntry {
        number: 0x0091,
        name: "NtAssignProcessToJobObject",
    },
    NtSyscallEntry {
        number: 0x0092,
        name: "NtAssociateWaitCompletionPacket",
    },
    NtSyscallEntry {
        number: 0x0093,
        name: "NtCallEnclave",
    },
    NtSyscallEntry {
        number: 0x0094,
        name: "NtCancelIoFileEx",
    },
    NtSyscallEntry {
        number: 0x0095,
        name: "NtCancelSynchronousIoFile",
    },
    NtSyscallEntry {
        number: 0x0096,
        name: "NtCancelTimer2",
    },
    NtSyscallEntry {
        number: 0x0097,
        name: "NtCancelWaitCompletionPacket",
    },
    NtSyscallEntry {
        number: 0x0098,
        name: "NtChangeProcessState",
    },
    NtSyscallEntry {
        number: 0x0099,
        name: "NtChangeThreadState",
    },
    NtSyscallEntry {
        number: 0x009A,
        name: "NtCommitComplete",
    },
    NtSyscallEntry {
        number: 0x009B,
        name: "NtCommitEnlistment",
    },
    NtSyscallEntry {
        number: 0x009C,
        name: "NtCommitRegistryTransaction",
    },
    NtSyscallEntry {
        number: 0x009D,
        name: "NtCommitTransaction",
    },
    NtSyscallEntry {
        number: 0x009E,
        name: "NtCompactKeys",
    },
    NtSyscallEntry {
        number: 0x009F,
        name: "NtCompareObjects",
    },
    NtSyscallEntry {
        number: 0x00A0,
        name: "NtCompareSigningLevels",
    },
    NtSyscallEntry {
        number: 0x00A1,
        name: "NtCompareTokens",
    },
    NtSyscallEntry {
        number: 0x00A2,
        name: "NtCompleteConnectPort",
    },
    NtSyscallEntry {
        number: 0x00A3,
        name: "NtCompressKey",
    },
    NtSyscallEntry {
        number: 0x00A4,
        name: "NtConnectPort",
    },
    NtSyscallEntry {
        number: 0x00A5,
        name: "NtContinueEx",
    },
    NtSyscallEntry {
        number: 0x00A6,
        name: "NtConvertBetweenAuxiliaryCounterAndPerformanceCounter",
    },
    NtSyscallEntry {
        number: 0x00A7,
        name: "NtCopyFileChunk",
    },
    NtSyscallEntry {
        number: 0x00A8,
        name: "NtCreateCpuPartition",
    },
    NtSyscallEntry {
        number: 0x00A9,
        name: "NtCreateCrossVmEvent",
    },
    NtSyscallEntry {
        number: 0x00AA,
        name: "NtCreateCrossVmMutant",
    },
    NtSyscallEntry {
        number: 0x00AB,
        name: "NtCreateDebugObject",
    },
    NtSyscallEntry {
        number: 0x00AC,
        name: "NtCreateDirectoryObject",
    },
    NtSyscallEntry {
        number: 0x00AD,
        name: "NtCreateDirectoryObjectEx",
    },
    NtSyscallEntry {
        number: 0x00AE,
        name: "NtCreateEnclave",
    },
    NtSyscallEntry {
        number: 0x00AF,
        name: "NtCreateEnlistment",
    },
    NtSyscallEntry {
        number: 0x00B0,
        name: "NtCreateEventPair",
    },
    NtSyscallEntry {
        number: 0x00B1,
        name: "NtCreateIRTimer",
    },
    NtSyscallEntry {
        number: 0x00B2,
        name: "NtCreateIoCompletion",
    },
    NtSyscallEntry {
        number: 0x00B3,
        name: "NtCreateIoRing",
    },
    NtSyscallEntry {
        number: 0x00B4,
        name: "NtCreateJobObject",
    },
    NtSyscallEntry {
        number: 0x00B5,
        name: "NtCreateJobSet",
    },
    NtSyscallEntry {
        number: 0x00B6,
        name: "NtCreateKeyTransacted",
    },
    NtSyscallEntry {
        number: 0x00B7,
        name: "NtCreateKeyedEvent",
    },
    NtSyscallEntry {
        number: 0x00B8,
        name: "NtCreateLowBoxToken",
    },
    NtSyscallEntry {
        number: 0x00B9,
        name: "NtCreateMailslotFile",
    },
    NtSyscallEntry {
        number: 0x00BA,
        name: "NtCreateMutant",
    },
    NtSyscallEntry {
        number: 0x00BB,
        name: "NtCreateNamedPipeFile",
    },
    NtSyscallEntry {
        number: 0x00BC,
        name: "NtCreatePagingFile",
    },
    NtSyscallEntry {
        number: 0x00BD,
        name: "NtCreatePartition",
    },
    NtSyscallEntry {
        number: 0x00BE,
        name: "NtCreatePort",
    },
    NtSyscallEntry {
        number: 0x00BF,
        name: "NtCreatePrivateNamespace",
    },
    NtSyscallEntry {
        number: 0x00C0,
        name: "NtCreateProcess",
    },
    NtSyscallEntry {
        number: 0x00C1,
        name: "NtCreateProcessStateChange",
    },
    NtSyscallEntry {
        number: 0x00C2,
        name: "NtCreateProfile",
    },
    NtSyscallEntry {
        number: 0x00C3,
        name: "NtCreateProfileEx",
    },
    NtSyscallEntry {
        number: 0x00C4,
        name: "NtCreateRegistryTransaction",
    },
    NtSyscallEntry {
        number: 0x00C5,
        name: "NtCreateResourceManager",
    },
    NtSyscallEntry {
        number: 0x00C6,
        name: "NtCreateSectionEx",
    },
    NtSyscallEntry {
        number: 0x00C7,
        name: "NtCreateSemaphore",
    },
    NtSyscallEntry {
        number: 0x00C8,
        name: "NtCreateSymbolicLinkObject",
    },
    NtSyscallEntry {
        number: 0x00C9,
        name: "NtCreateThreadEx",
    },
    NtSyscallEntry {
        number: 0x00CA,
        name: "NtCreateThreadStateChange",
    },
    NtSyscallEntry {
        number: 0x00CB,
        name: "NtCreateTimer",
    },
    NtSyscallEntry {
        number: 0x00CC,
        name: "NtCreateTimer2",
    },
    NtSyscallEntry {
        number: 0x00CD,
        name: "NtCreateToken",
    },
    NtSyscallEntry {
        number: 0x00CE,
        name: "NtCreateTokenEx",
    },
    NtSyscallEntry {
        number: 0x00CF,
        name: "NtCreateTransaction",
    },
    NtSyscallEntry {
        number: 0x00D0,
        name: "NtCreateTransactionManager",
    },
    NtSyscallEntry {
        number: 0x00D1,
        name: "NtCreateUserProcess",
    },
    NtSyscallEntry {
        number: 0x00D2,
        name: "NtCreateWaitCompletionPacket",
    },
    NtSyscallEntry {
        number: 0x00D3,
        name: "NtCreateWaitablePort",
    },
    NtSyscallEntry {
        number: 0x00D4,
        name: "NtCreateWnfStateName",
    },
    NtSyscallEntry {
        number: 0x00D5,
        name: "NtCreateWorkerFactory",
    },
    NtSyscallEntry {
        number: 0x00D6,
        name: "NtDebugActiveProcess",
    },
    NtSyscallEntry {
        number: 0x00D7,
        name: "NtDebugContinue",
    },
    NtSyscallEntry {
        number: 0x00D8,
        name: "NtDeleteAtom",
    },
    NtSyscallEntry {
        number: 0x00D9,
        name: "NtDeleteBootEntry",
    },
    NtSyscallEntry {
        number: 0x00DA,
        name: "NtDeleteDriverEntry",
    },
    NtSyscallEntry {
        number: 0x00DB,
        name: "NtDeleteFile",
    },
    NtSyscallEntry {
        number: 0x00DC,
        name: "NtDeleteKey",
    },
    NtSyscallEntry {
        number: 0x00DD,
        name: "NtDeleteObjectAuditAlarm",
    },
    NtSyscallEntry {
        number: 0x00DE,
        name: "NtDeletePrivateNamespace",
    },
    NtSyscallEntry {
        number: 0x00DF,
        name: "NtDeleteValueKey",
    },
    NtSyscallEntry {
        number: 0x00E0,
        name: "NtDeleteWnfStateData",
    },
    NtSyscallEntry {
        number: 0x00E1,
        name: "NtDeleteWnfStateName",
    },
    NtSyscallEntry {
        number: 0x00E2,
        name: "NtDirectGraphicsCall",
    },
    NtSyscallEntry {
        number: 0x00E3,
        name: "NtDisableLastKnownGood",
    },
    NtSyscallEntry {
        number: 0x00E4,
        name: "NtDisplayString",
    },
    NtSyscallEntry {
        number: 0x00E5,
        name: "NtDrawText",
    },
    NtSyscallEntry {
        number: 0x00E6,
        name: "NtEnableLastKnownGood",
    },
    NtSyscallEntry {
        number: 0x00E7,
        name: "NtEnumerateBootEntries",
    },
    NtSyscallEntry {
        number: 0x00E8,
        name: "NtEnumerateDriverEntries",
    },
    NtSyscallEntry {
        number: 0x00E9,
        name: "NtEnumerateSystemEnvironmentValuesEx",
    },
    NtSyscallEntry {
        number: 0x00EA,
        name: "NtEnumerateTransactionObject",
    },
    NtSyscallEntry {
        number: 0x00EB,
        name: "NtExtendSection",
    },
    NtSyscallEntry {
        number: 0x00EC,
        name: "NtFilterBootOption",
    },
    NtSyscallEntry {
        number: 0x00ED,
        name: "NtFilterToken",
    },
    NtSyscallEntry {
        number: 0x00EE,
        name: "NtFilterTokenEx",
    },
    NtSyscallEntry {
        number: 0x00EF,
        name: "NtFlushBuffersFileEx",
    },
    NtSyscallEntry {
        number: 0x00F0,
        name: "NtFlushInstallUILanguage",
    },
    NtSyscallEntry {
        number: 0x00F1,
        name: "NtFlushInstructionCache",
    },
    NtSyscallEntry {
        number: 0x00F2,
        name: "NtFlushKey",
    },
    NtSyscallEntry {
        number: 0x00F3,
        name: "NtFlushProcessWriteBuffers",
    },
    NtSyscallEntry {
        number: 0x00F4,
        name: "NtFlushVirtualMemory",
    },
    NtSyscallEntry {
        number: 0x00F5,
        name: "NtFlushWriteBuffer",
    },
    NtSyscallEntry {
        number: 0x00F6,
        name: "NtFreeUserPhysicalPages",
    },
    NtSyscallEntry {
        number: 0x00F7,
        name: "NtFreezeRegistry",
    },
    NtSyscallEntry {
        number: 0x00F8,
        name: "NtFreezeTransactions",
    },
    NtSyscallEntry {
        number: 0x00F9,
        name: "NtGetCachedSigningLevel",
    },
    NtSyscallEntry {
        number: 0x00FA,
        name: "NtGetCompleteWnfStateSubscription",
    },
    NtSyscallEntry {
        number: 0x00FB,
        name: "NtGetContextThread",
    },
    NtSyscallEntry {
        number: 0x00FC,
        name: "NtGetCurrentProcessorNumber",
    },
    NtSyscallEntry {
        number: 0x00FD,
        name: "NtGetCurrentProcessorNumberEx",
    },
    NtSyscallEntry {
        number: 0x00FE,
        name: "NtGetDevicePowerState",
    },
    NtSyscallEntry {
        number: 0x00FF,
        name: "NtGetMUIRegistryInfo",
    },
    NtSyscallEntry {
        number: 0x0100,
        name: "NtGetNextProcess",
    },
    NtSyscallEntry {
        number: 0x0101,
        name: "NtGetNextThread",
    },
    NtSyscallEntry {
        number: 0x0102,
        name: "NtGetNlsSectionPtr",
    },
    NtSyscallEntry {
        number: 0x0103,
        name: "NtGetNotificationResourceManager",
    },
    NtSyscallEntry {
        number: 0x0104,
        name: "NtGetWriteWatch",
    },
    NtSyscallEntry {
        number: 0x0105,
        name: "NtImpersonateAnonymousToken",
    },
    NtSyscallEntry {
        number: 0x0106,
        name: "NtImpersonateThread",
    },
    NtSyscallEntry {
        number: 0x0107,
        name: "NtInitializeEnclave",
    },
    NtSyscallEntry {
        number: 0x0108,
        name: "NtInitializeNlsFiles",
    },
    NtSyscallEntry {
        number: 0x0109,
        name: "NtInitializeRegistry",
    },
    NtSyscallEntry {
        number: 0x010A,
        name: "NtInitiatePowerAction",
    },
    NtSyscallEntry {
        number: 0x010B,
        name: "NtIsSystemResumeAutomatic",
    },
    NtSyscallEntry {
        number: 0x010C,
        name: "NtIsUILanguageComitted",
    },
    NtSyscallEntry {
        number: 0x010D,
        name: "NtListenPort",
    },
    NtSyscallEntry {
        number: 0x010E,
        name: "NtLoadDriver",
    },
    NtSyscallEntry {
        number: 0x010F,
        name: "NtLoadEnclaveData",
    },
    NtSyscallEntry {
        number: 0x0110,
        name: "NtLoadKey",
    },
    NtSyscallEntry {
        number: 0x0111,
        name: "NtLoadKey2",
    },
    NtSyscallEntry {
        number: 0x0112,
        name: "NtLoadKey3",
    },
    NtSyscallEntry {
        number: 0x0113,
        name: "NtLoadKeyEx",
    },
    NtSyscallEntry {
        number: 0x0114,
        name: "NtLockFile",
    },
    NtSyscallEntry {
        number: 0x0115,
        name: "NtLockProductActivationKeys",
    },
    NtSyscallEntry {
        number: 0x0116,
        name: "NtLockRegistryKey",
    },
    NtSyscallEntry {
        number: 0x0117,
        name: "NtLockVirtualMemory",
    },
    NtSyscallEntry {
        number: 0x0118,
        name: "NtMakePermanentObject",
    },
    NtSyscallEntry {
        number: 0x0119,
        name: "NtMakeTemporaryObject",
    },
    NtSyscallEntry {
        number: 0x011A,
        name: "NtManageHotPatch",
    },
    NtSyscallEntry {
        number: 0x011B,
        name: "NtManagePartition",
    },
    NtSyscallEntry {
        number: 0x011C,
        name: "NtMapCMFModule",
    },
    NtSyscallEntry {
        number: 0x011D,
        name: "NtMapUserPhysicalPages",
    },
    NtSyscallEntry {
        number: 0x011E,
        name: "NtMapViewOfSectionEx",
    },
    NtSyscallEntry {
        number: 0x011F,
        name: "NtModifyBootEntry",
    },
    NtSyscallEntry {
        number: 0x0120,
        name: "NtModifyDriverEntry",
    },
    NtSyscallEntry {
        number: 0x0121,
        name: "NtNotifyChangeDirectoryFile",
    },
    NtSyscallEntry {
        number: 0x0122,
        name: "NtNotifyChangeDirectoryFileEx",
    },
    NtSyscallEntry {
        number: 0x0123,
        name: "NtNotifyChangeKey",
    },
    NtSyscallEntry {
        number: 0x0124,
        name: "NtNotifyChangeMultipleKeys",
    },
    NtSyscallEntry {
        number: 0x0125,
        name: "NtNotifyChangeSession",
    },
    NtSyscallEntry {
        number: 0x0126,
        name: "NtOpenCpuPartition",
    },
    NtSyscallEntry {
        number: 0x0127,
        name: "NtOpenEnlistment",
    },
    NtSyscallEntry {
        number: 0x0128,
        name: "NtOpenEventPair",
    },
    NtSyscallEntry {
        number: 0x0129,
        name: "NtOpenIoCompletion",
    },
    NtSyscallEntry {
        number: 0x012A,
        name: "NtOpenJobObject",
    },
    NtSyscallEntry {
        number: 0x012B,
        name: "NtOpenKeyEx",
    },
    NtSyscallEntry {
        number: 0x012C,
        name: "NtOpenKeyTransacted",
    },
    NtSyscallEntry {
        number: 0x012D,
        name: "NtOpenKeyTransactedEx",
    },
    NtSyscallEntry {
        number: 0x012E,
        name: "NtOpenKeyedEvent",
    },
    NtSyscallEntry {
        number: 0x012F,
        name: "NtOpenMutant",
    },
    NtSyscallEntry {
        number: 0x0130,
        name: "NtOpenObjectAuditAlarm",
    },
    NtSyscallEntry {
        number: 0x0131,
        name: "NtOpenPartition",
    },
    NtSyscallEntry {
        number: 0x0132,
        name: "NtOpenPrivateNamespace",
    },
    NtSyscallEntry {
        number: 0x0133,
        name: "NtOpenProcessToken",
    },
    NtSyscallEntry {
        number: 0x0134,
        name: "NtOpenRegistryTransaction",
    },
    NtSyscallEntry {
        number: 0x0135,
        name: "NtOpenResourceManager",
    },
    NtSyscallEntry {
        number: 0x0136,
        name: "NtOpenSemaphore",
    },
    NtSyscallEntry {
        number: 0x0137,
        name: "NtOpenSession",
    },
    NtSyscallEntry {
        number: 0x0138,
        name: "NtOpenSymbolicLinkObject",
    },
    NtSyscallEntry {
        number: 0x0139,
        name: "NtOpenThread",
    },
    NtSyscallEntry {
        number: 0x013A,
        name: "NtOpenTimer",
    },
    NtSyscallEntry {
        number: 0x013B,
        name: "NtOpenTransaction",
    },
    NtSyscallEntry {
        number: 0x013C,
        name: "NtOpenTransactionManager",
    },
    NtSyscallEntry {
        number: 0x013D,
        name: "NtPlugPlayControl",
    },
    NtSyscallEntry {
        number: 0x013E,
        name: "NtPrePrepareComplete",
    },
    NtSyscallEntry {
        number: 0x013F,
        name: "NtPrePrepareEnlistment",
    },
    NtSyscallEntry {
        number: 0x0140,
        name: "NtPrepareComplete",
    },
    NtSyscallEntry {
        number: 0x0141,
        name: "NtPrepareEnlistment",
    },
    NtSyscallEntry {
        number: 0x0142,
        name: "NtPrivilegeCheck",
    },
    NtSyscallEntry {
        number: 0x0143,
        name: "NtPrivilegeObjectAuditAlarm",
    },
    NtSyscallEntry {
        number: 0x0144,
        name: "NtPrivilegedServiceAuditAlarm",
    },
    NtSyscallEntry {
        number: 0x0145,
        name: "NtPropagationComplete",
    },
    NtSyscallEntry {
        number: 0x0146,
        name: "NtPropagationFailed",
    },
    NtSyscallEntry {
        number: 0x0147,
        name: "NtPssCaptureVaSpaceBulk",
    },
    NtSyscallEntry {
        number: 0x0148,
        name: "NtPulseEvent",
    },
    NtSyscallEntry {
        number: 0x0149,
        name: "NtQueryAuxiliaryCounterFrequency",
    },
    NtSyscallEntry {
        number: 0x014A,
        name: "NtQueryBootEntryOrder",
    },
    NtSyscallEntry {
        number: 0x014B,
        name: "NtQueryBootOptions",
    },
    NtSyscallEntry {
        number: 0x014C,
        name: "NtQueryDebugFilterState",
    },
    NtSyscallEntry {
        number: 0x014D,
        name: "NtQueryDirectoryFileEx",
    },
    NtSyscallEntry {
        number: 0x014E,
        name: "NtQueryDirectoryObject",
    },
    NtSyscallEntry {
        number: 0x014F,
        name: "NtQueryDriverEntryOrder",
    },
    NtSyscallEntry {
        number: 0x0150,
        name: "NtQueryEaFile",
    },
    NtSyscallEntry {
        number: 0x0151,
        name: "NtQueryFullAttributesFile",
    },
    NtSyscallEntry {
        number: 0x0152,
        name: "NtQueryInformationAtom",
    },
    NtSyscallEntry {
        number: 0x0153,
        name: "NtQueryInformationByName",
    },
    NtSyscallEntry {
        number: 0x0154,
        name: "NtQueryInformationCpuPartition",
    },
    NtSyscallEntry {
        number: 0x0155,
        name: "NtQueryInformationEnlistment",
    },
    NtSyscallEntry {
        number: 0x0156,
        name: "NtQueryInformationJobObject",
    },
    NtSyscallEntry {
        number: 0x0157,
        name: "NtQueryInformationPort",
    },
    NtSyscallEntry {
        number: 0x0158,
        name: "NtQueryInformationResourceManager",
    },
    NtSyscallEntry {
        number: 0x0159,
        name: "NtQueryInformationTransaction",
    },
    NtSyscallEntry {
        number: 0x015A,
        name: "NtQueryInformationTransactionManager",
    },
    NtSyscallEntry {
        number: 0x015B,
        name: "NtQueryInformationWorkerFactory",
    },
    NtSyscallEntry {
        number: 0x015C,
        name: "NtQueryInstallUILanguage",
    },
    NtSyscallEntry {
        number: 0x015D,
        name: "NtQueryIntervalProfile",
    },
    NtSyscallEntry {
        number: 0x015E,
        name: "NtQueryIoCompletion",
    },
    NtSyscallEntry {
        number: 0x015F,
        name: "NtQueryIoRingCapabilities",
    },
    NtSyscallEntry {
        number: 0x0160,
        name: "NtQueryLicenseValue",
    },
    NtSyscallEntry {
        number: 0x0161,
        name: "NtQueryMultipleValueKey",
    },
    NtSyscallEntry {
        number: 0x0162,
        name: "NtQueryMutant",
    },
    NtSyscallEntry {
        number: 0x0163,
        name: "NtQueryOpenSubKeys",
    },
    NtSyscallEntry {
        number: 0x0164,
        name: "NtQueryOpenSubKeysEx",
    },
    NtSyscallEntry {
        number: 0x0165,
        name: "NtQueryPortInformationProcess",
    },
    NtSyscallEntry {
        number: 0x0166,
        name: "NtQueryQuotaInformationFile",
    },
    NtSyscallEntry {
        number: 0x0167,
        name: "NtQuerySecurityAttributesToken",
    },
    NtSyscallEntry {
        number: 0x0168,
        name: "NtQuerySecurityObject",
    },
    NtSyscallEntry {
        number: 0x0169,
        name: "NtQuerySecurityPolicy",
    },
    NtSyscallEntry {
        number: 0x016A,
        name: "NtQuerySemaphore",
    },
    NtSyscallEntry {
        number: 0x016B,
        name: "NtQuerySymbolicLinkObject",
    },
    NtSyscallEntry {
        number: 0x016C,
        name: "NtQuerySystemEnvironmentValue",
    },
    NtSyscallEntry {
        number: 0x016D,
        name: "NtQuerySystemEnvironmentValueEx",
    },
    NtSyscallEntry {
        number: 0x016E,
        name: "NtQuerySystemInformationEx",
    },
    NtSyscallEntry {
        number: 0x016F,
        name: "NtQueryTimerResolution",
    },
    NtSyscallEntry {
        number: 0x0170,
        name: "NtQueryWnfStateData",
    },
    NtSyscallEntry {
        number: 0x0171,
        name: "NtQueryWnfStateNameInformation",
    },
    NtSyscallEntry {
        number: 0x0172,
        name: "NtQueueApcThreadEx",
    },
    NtSyscallEntry {
        number: 0x0173,
        name: "NtQueueApcThreadEx2",
    },
    NtSyscallEntry {
        number: 0x0174,
        name: "NtRaiseException",
    },
    NtSyscallEntry {
        number: 0x0175,
        name: "NtRaiseHardError",
    },
    NtSyscallEntry {
        number: 0x0176,
        name: "NtReadOnlyEnlistment",
    },
    NtSyscallEntry {
        number: 0x0177,
        name: "NtReadVirtualMemoryEx",
    },
    NtSyscallEntry {
        number: 0x0178,
        name: "NtRecoverEnlistment",
    },
    NtSyscallEntry {
        number: 0x0179,
        name: "NtRecoverResourceManager",
    },
    NtSyscallEntry {
        number: 0x017A,
        name: "NtRecoverTransactionManager",
    },
    NtSyscallEntry {
        number: 0x017B,
        name: "NtRegisterProtocolAddressInformation",
    },
    NtSyscallEntry {
        number: 0x017C,
        name: "NtRegisterThreadTerminatePort",
    },
    NtSyscallEntry {
        number: 0x017D,
        name: "NtReleaseKeyedEvent",
    },
    NtSyscallEntry {
        number: 0x017E,
        name: "NtReleaseWorkerFactoryWorker",
    },
    NtSyscallEntry {
        number: 0x017F,
        name: "NtRemoveIoCompletionEx",
    },
    NtSyscallEntry {
        number: 0x0180,
        name: "NtRemoveProcessDebug",
    },
    NtSyscallEntry {
        number: 0x0181,
        name: "NtRenameKey",
    },
    NtSyscallEntry {
        number: 0x0182,
        name: "NtRenameTransactionManager",
    },
    NtSyscallEntry {
        number: 0x0183,
        name: "NtReplaceKey",
    },
    NtSyscallEntry {
        number: 0x0184,
        name: "NtReplacePartitionUnit",
    },
    NtSyscallEntry {
        number: 0x0185,
        name: "NtReplyWaitReplyPort",
    },
    NtSyscallEntry {
        number: 0x0186,
        name: "NtRequestPort",
    },
    NtSyscallEntry {
        number: 0x0187,
        name: "NtResetEvent",
    },
    NtSyscallEntry {
        number: 0x0188,
        name: "NtResetWriteWatch",
    },
    NtSyscallEntry {
        number: 0x0189,
        name: "NtRestoreKey",
    },
    NtSyscallEntry {
        number: 0x018A,
        name: "NtResumeProcess",
    },
    NtSyscallEntry {
        number: 0x018B,
        name: "NtRevertContainerImpersonation",
    },
    NtSyscallEntry {
        number: 0x018C,
        name: "NtRollbackComplete",
    },
    NtSyscallEntry {
        number: 0x018D,
        name: "NtRollbackEnlistment",
    },
    NtSyscallEntry {
        number: 0x018E,
        name: "NtRollbackRegistryTransaction",
    },
    NtSyscallEntry {
        number: 0x018F,
        name: "NtRollbackTransaction",
    },
    NtSyscallEntry {
        number: 0x0190,
        name: "NtRollforwardTransactionManager",
    },
    NtSyscallEntry {
        number: 0x0191,
        name: "NtSaveKey",
    },
    NtSyscallEntry {
        number: 0x0192,
        name: "NtSaveKeyEx",
    },
    NtSyscallEntry {
        number: 0x0193,
        name: "NtSaveMergedKeys",
    },
    NtSyscallEntry {
        number: 0x0194,
        name: "NtSecureConnectPort",
    },
    NtSyscallEntry {
        number: 0x0195,
        name: "NtSerializeBoot",
    },
    NtSyscallEntry {
        number: 0x0196,
        name: "NtSetBootEntryOrder",
    },
    NtSyscallEntry {
        number: 0x0197,
        name: "NtSetBootOptions",
    },
    NtSyscallEntry {
        number: 0x0198,
        name: "NtSetCachedSigningLevel",
    },
    NtSyscallEntry {
        number: 0x0199,
        name: "NtSetCachedSigningLevel2",
    },
    NtSyscallEntry {
        number: 0x019A,
        name: "NtSetContextThread",
    },
    NtSyscallEntry {
        number: 0x019B,
        name: "NtSetDebugFilterState",
    },
    NtSyscallEntry {
        number: 0x019C,
        name: "NtSetDefaultHardErrorPort",
    },
    NtSyscallEntry {
        number: 0x019D,
        name: "NtSetDefaultLocale",
    },
    NtSyscallEntry {
        number: 0x019E,
        name: "NtSetDefaultUILanguage",
    },
    NtSyscallEntry {
        number: 0x019F,
        name: "NtSetDriverEntryOrder",
    },
    NtSyscallEntry {
        number: 0x01A0,
        name: "NtSetEaFile",
    },
    NtSyscallEntry {
        number: 0x01A1,
        name: "NtSetEventEx",
    },
    NtSyscallEntry {
        number: 0x01A2,
        name: "NtSetHighEventPair",
    },
    NtSyscallEntry {
        number: 0x01A3,
        name: "NtSetHighWaitLowEventPair",
    },
    NtSyscallEntry {
        number: 0x01A4,
        name: "NtSetIRTimer",
    },
    NtSyscallEntry {
        number: 0x01A5,
        name: "NtSetInformationCpuPartition",
    },
    NtSyscallEntry {
        number: 0x01A6,
        name: "NtSetInformationDebugObject",
    },
    NtSyscallEntry {
        number: 0x01A7,
        name: "NtSetInformationEnlistment",
    },
    NtSyscallEntry {
        number: 0x01A8,
        name: "NtSetInformationIoRing",
    },
    NtSyscallEntry {
        number: 0x01A9,
        name: "NtSetInformationJobObject",
    },
    NtSyscallEntry {
        number: 0x01AA,
        name: "NtSetInformationKey",
    },
    NtSyscallEntry {
        number: 0x01AB,
        name: "NtSetInformationResourceManager",
    },
    NtSyscallEntry {
        number: 0x01AC,
        name: "NtSetInformationSymbolicLink",
    },
    NtSyscallEntry {
        number: 0x01AD,
        name: "NtSetInformationToken",
    },
    NtSyscallEntry {
        number: 0x01AE,
        name: "NtSetInformationTransaction",
    },
    NtSyscallEntry {
        number: 0x01AF,
        name: "NtSetInformationTransactionManager",
    },
    NtSyscallEntry {
        number: 0x01B0,
        name: "NtSetInformationVirtualMemory",
    },
    NtSyscallEntry {
        number: 0x01B1,
        name: "NtSetInformationWorkerFactory",
    },
    NtSyscallEntry {
        number: 0x01B2,
        name: "NtSetIntervalProfile",
    },
    NtSyscallEntry {
        number: 0x01B3,
        name: "NtSetIoCompletion",
    },
    NtSyscallEntry {
        number: 0x01B4,
        name: "NtSetIoCompletionEx",
    },
    NtSyscallEntry {
        number: 0x01B5,
        name: "NtSetLdtEntries",
    },
    NtSyscallEntry {
        number: 0x01B6,
        name: "NtSetLowEventPair",
    },
    NtSyscallEntry {
        number: 0x01B7,
        name: "NtSetLowWaitHighEventPair",
    },
    NtSyscallEntry {
        number: 0x01B8,
        name: "NtSetQuotaInformationFile",
    },
    NtSyscallEntry {
        number: 0x01B9,
        name: "NtSetSecurityObject",
    },
    NtSyscallEntry {
        number: 0x01BA,
        name: "NtSetSystemEnvironmentValue",
    },
    NtSyscallEntry {
        number: 0x01BB,
        name: "NtSetSystemEnvironmentValueEx",
    },
    NtSyscallEntry {
        number: 0x01BC,
        name: "NtSetSystemInformation",
    },
    NtSyscallEntry {
        number: 0x01BD,
        name: "NtSetSystemPowerState",
    },
    NtSyscallEntry {
        number: 0x01BE,
        name: "NtSetSystemTime",
    },
    NtSyscallEntry {
        number: 0x01BF,
        name: "NtSetThreadExecutionState",
    },
    NtSyscallEntry {
        number: 0x01C0,
        name: "NtSetTimer2",
    },
    NtSyscallEntry {
        number: 0x01C1,
        name: "NtSetTimerEx",
    },
    NtSyscallEntry {
        number: 0x01C2,
        name: "NtSetTimerResolution",
    },
    NtSyscallEntry {
        number: 0x01C3,
        name: "NtSetUuidSeed",
    },
    NtSyscallEntry {
        number: 0x01C4,
        name: "NtSetVolumeInformationFile",
    },
    NtSyscallEntry {
        number: 0x01C5,
        name: "NtSetWnfProcessNotificationEvent",
    },
    NtSyscallEntry {
        number: 0x01C6,
        name: "NtShutdownSystem",
    },
    NtSyscallEntry {
        number: 0x01C7,
        name: "NtShutdownWorkerFactory",
    },
    NtSyscallEntry {
        number: 0x01C8,
        name: "NtSignalAndWaitForSingleObject",
    },
    NtSyscallEntry {
        number: 0x01C9,
        name: "NtSinglePhaseReject",
    },
    NtSyscallEntry {
        number: 0x01CA,
        name: "NtStartProfile",
    },
    NtSyscallEntry {
        number: 0x01CB,
        name: "NtStopProfile",
    },
    NtSyscallEntry {
        number: 0x01CC,
        name: "NtSubmitIoRing",
    },
    NtSyscallEntry {
        number: 0x01CD,
        name: "NtSubscribeWnfStateChange",
    },
    NtSyscallEntry {
        number: 0x01CE,
        name: "NtSuspendProcess",
    },
    NtSyscallEntry {
        number: 0x01CF,
        name: "NtSuspendThread",
    },
    NtSyscallEntry {
        number: 0x01D0,
        name: "NtSystemDebugControl",
    },
    NtSyscallEntry {
        number: 0x01D1,
        name: "NtTerminateEnclave",
    },
    NtSyscallEntry {
        number: 0x01D2,
        name: "NtTerminateJobObject",
    },
    NtSyscallEntry {
        number: 0x01D3,
        name: "NtTestAlert",
    },
    NtSyscallEntry {
        number: 0x01D4,
        name: "NtThawRegistry",
    },
    NtSyscallEntry {
        number: 0x01D5,
        name: "NtThawTransactions",
    },
    NtSyscallEntry {
        number: 0x01D6,
        name: "NtTraceControl",
    },
    NtSyscallEntry {
        number: 0x01D7,
        name: "NtTranslateFilePath",
    },
    NtSyscallEntry {
        number: 0x01D8,
        name: "NtUmsThreadYield",
    },
    NtSyscallEntry {
        number: 0x01D9,
        name: "NtUnloadDriver",
    },
    NtSyscallEntry {
        number: 0x01DA,
        name: "NtUnloadKey",
    },
    NtSyscallEntry {
        number: 0x01DB,
        name: "NtUnloadKey2",
    },
    NtSyscallEntry {
        number: 0x01DC,
        name: "NtUnloadKeyEx",
    },
    NtSyscallEntry {
        number: 0x01DD,
        name: "NtUnlockFile",
    },
    NtSyscallEntry {
        number: 0x01DE,
        name: "NtUnlockVirtualMemory",
    },
    NtSyscallEntry {
        number: 0x01DF,
        name: "NtUnmapViewOfSectionEx",
    },
    NtSyscallEntry {
        number: 0x01E0,
        name: "NtUnsubscribeWnfStateChange",
    },
    NtSyscallEntry {
        number: 0x01E1,
        name: "NtUpdateWnfStateData",
    },
    NtSyscallEntry {
        number: 0x01E2,
        name: "NtVdmControl",
    },
    NtSyscallEntry {
        number: 0x01E3,
        name: "NtWaitForAlertByThreadId",
    },
    NtSyscallEntry {
        number: 0x01E4,
        name: "NtWaitForDebugEvent",
    },
    NtSyscallEntry {
        number: 0x01E5,
        name: "NtWaitForKeyedEvent",
    },
    NtSyscallEntry {
        number: 0x01E6,
        name: "NtWaitForWorkViaWorkerFactory",
    },
    NtSyscallEntry {
        number: 0x01E7,
        name: "NtWaitHighEventPair",
    },
    NtSyscallEntry {
        number: 0x01E8,
        name: "NtWaitLowEventPair",
    },
];

const _NT_SYSCALLS_BY_NAME: &[NtSyscallEntry] = &[
    NtSyscallEntry {
        number: 0x0002,
        name: "NtAcceptConnectPort",
    },
    NtSyscallEntry {
        number: 0x0000,
        name: "NtAccessCheck",
    },
    NtSyscallEntry {
        number: 0x0029,
        name: "NtAccessCheckAndAuditAlarm",
    },
    NtSyscallEntry {
        number: 0x0063,
        name: "NtAccessCheckByType",
    },
    NtSyscallEntry {
        number: 0x0059,
        name: "NtAccessCheckByTypeAndAuditAlarm",
    },
    NtSyscallEntry {
        number: 0x0064,
        name: "NtAccessCheckByTypeResultList",
    },
    NtSyscallEntry {
        number: 0x0065,
        name: "NtAccessCheckByTypeResultListAndAuditAlarm",
    },
    NtSyscallEntry {
        number: 0x0066,
        name: "NtAccessCheckByTypeResultListAndAuditAlarmByHandle",
    },
    NtSyscallEntry {
        number: 0x0067,
        name: "NtAcquireCrossVmMutant",
    },
    NtSyscallEntry {
        number: 0x0068,
        name: "NtAcquireProcessActivityReference",
    },
    NtSyscallEntry {
        number: 0x0047,
        name: "NtAddAtom",
    },
    NtSyscallEntry {
        number: 0x0069,
        name: "NtAddAtomEx",
    },
    NtSyscallEntry {
        number: 0x006A,
        name: "NtAddBootEntry",
    },
    NtSyscallEntry {
        number: 0x006B,
        name: "NtAddDriverEntry",
    },
    NtSyscallEntry {
        number: 0x006C,
        name: "NtAdjustGroupsToken",
    },
    NtSyscallEntry {
        number: 0x0041,
        name: "NtAdjustPrivilegesToken",
    },
    NtSyscallEntry {
        number: 0x006D,
        name: "NtAdjustTokenClaimsAndDeviceGroups",
    },
    NtSyscallEntry {
        number: 0x006E,
        name: "NtAlertMultipleThreadByThreadId",
    },
    NtSyscallEntry {
        number: 0x006F,
        name: "NtAlertResumeThread",
    },
    NtSyscallEntry {
        number: 0x0070,
        name: "NtAlertThread",
    },
    NtSyscallEntry {
        number: 0x0071,
        name: "NtAlertThreadByThreadId",
    },
    NtSyscallEntry {
        number: 0x0072,
        name: "NtAlertThreadByThreadIdEx",
    },
    NtSyscallEntry {
        number: 0x0073,
        name: "NtAllocateLocallyUniqueId",
    },
    NtSyscallEntry {
        number: 0x0074,
        name: "NtAllocateReserveObject",
    },
    NtSyscallEntry {
        number: 0x0075,
        name: "NtAllocateUserPhysicalPages",
    },
    NtSyscallEntry {
        number: 0x0076,
        name: "NtAllocateUserPhysicalPagesEx",
    },
    NtSyscallEntry {
        number: 0x0077,
        name: "NtAllocateUuids",
    },
    NtSyscallEntry {
        number: 0x0018,
        name: "NtAllocateVirtualMemory",
    },
    NtSyscallEntry {
        number: 0x0078,
        name: "NtAllocateVirtualMemoryEx",
    },
    NtSyscallEntry {
        number: 0x0079,
        name: "NtAlpcAcceptConnectPort",
    },
    NtSyscallEntry {
        number: 0x007A,
        name: "NtAlpcCancelMessage",
    },
    NtSyscallEntry {
        number: 0x007B,
        name: "NtAlpcConnectPort",
    },
    NtSyscallEntry {
        number: 0x007C,
        name: "NtAlpcConnectPortEx",
    },
    NtSyscallEntry {
        number: 0x007D,
        name: "NtAlpcCreatePort",
    },
    NtSyscallEntry {
        number: 0x007E,
        name: "NtAlpcCreatePortSection",
    },
    NtSyscallEntry {
        number: 0x007F,
        name: "NtAlpcCreateResourceReserve",
    },
    NtSyscallEntry {
        number: 0x0080,
        name: "NtAlpcCreateSectionView",
    },
    NtSyscallEntry {
        number: 0x0081,
        name: "NtAlpcCreateSecurityContext",
    },
    NtSyscallEntry {
        number: 0x0082,
        name: "NtAlpcDeletePortSection",
    },
    NtSyscallEntry {
        number: 0x0083,
        name: "NtAlpcDeleteResourceReserve",
    },
    NtSyscallEntry {
        number: 0x0084,
        name: "NtAlpcDeleteSectionView",
    },
    NtSyscallEntry {
        number: 0x0085,
        name: "NtAlpcDeleteSecurityContext",
    },
    NtSyscallEntry {
        number: 0x0086,
        name: "NtAlpcDisconnectPort",
    },
    NtSyscallEntry {
        number: 0x0087,
        name: "NtAlpcImpersonateClientContainerOfPort",
    },
    NtSyscallEntry {
        number: 0x0088,
        name: "NtAlpcImpersonateClientOfPort",
    },
    NtSyscallEntry {
        number: 0x0089,
        name: "NtAlpcOpenSenderProcess",
    },
    NtSyscallEntry {
        number: 0x008A,
        name: "NtAlpcOpenSenderThread",
    },
    NtSyscallEntry {
        number: 0x008B,
        name: "NtAlpcQueryInformation",
    },
    NtSyscallEntry {
        number: 0x008C,
        name: "NtAlpcQueryInformationMessage",
    },
    NtSyscallEntry {
        number: 0x008D,
        name: "NtAlpcRevokeSecurityContext",
    },
    NtSyscallEntry {
        number: 0x008E,
        name: "NtAlpcSendWaitReceivePort",
    },
    NtSyscallEntry {
        number: 0x008F,
        name: "NtAlpcSetInformation",
    },
    NtSyscallEntry {
        number: 0x004C,
        name: "NtApphelpCacheControl",
    },
    NtSyscallEntry {
        number: 0x0090,
        name: "NtAreMappedFilesTheSame",
    },
    NtSyscallEntry {
        number: 0x0091,
        name: "NtAssignProcessToJobObject",
    },
    NtSyscallEntry {
        number: 0x0092,
        name: "NtAssociateWaitCompletionPacket",
    },
    NtSyscallEntry {
        number: 0x0093,
        name: "NtCallEnclave",
    },
    NtSyscallEntry {
        number: 0x0005,
        name: "NtCallbackReturn",
    },
    NtSyscallEntry {
        number: 0x005D,
        name: "NtCancelIoFile",
    },
    NtSyscallEntry {
        number: 0x0094,
        name: "NtCancelIoFileEx",
    },
    NtSyscallEntry {
        number: 0x0095,
        name: "NtCancelSynchronousIoFile",
    },
    NtSyscallEntry {
        number: 0x0061,
        name: "NtCancelTimer",
    },
    NtSyscallEntry {
        number: 0x0096,
        name: "NtCancelTimer2",
    },
    NtSyscallEntry {
        number: 0x0097,
        name: "NtCancelWaitCompletionPacket",
    },
    NtSyscallEntry {
        number: 0x0098,
        name: "NtChangeProcessState",
    },
    NtSyscallEntry {
        number: 0x0099,
        name: "NtChangeThreadState",
    },
    NtSyscallEntry {
        number: 0x003E,
        name: "NtClearEvent",
    },
    NtSyscallEntry {
        number: 0x000F,
        name: "NtClose",
    },
    NtSyscallEntry {
        number: 0x003B,
        name: "NtCloseObjectAuditAlarm",
    },
    NtSyscallEntry {
        number: 0x009A,
        name: "NtCommitComplete",
    },
    NtSyscallEntry {
        number: 0x009B,
        name: "NtCommitEnlistment",
    },
    NtSyscallEntry {
        number: 0x009C,
        name: "NtCommitRegistryTransaction",
    },
    NtSyscallEntry {
        number: 0x009D,
        name: "NtCommitTransaction",
    },
    NtSyscallEntry {
        number: 0x009E,
        name: "NtCompactKeys",
    },
    NtSyscallEntry {
        number: 0x009F,
        name: "NtCompareObjects",
    },
    NtSyscallEntry {
        number: 0x00A0,
        name: "NtCompareSigningLevels",
    },
    NtSyscallEntry {
        number: 0x00A1,
        name: "NtCompareTokens",
    },
    NtSyscallEntry {
        number: 0x00A2,
        name: "NtCompleteConnectPort",
    },
    NtSyscallEntry {
        number: 0x00A3,
        name: "NtCompressKey",
    },
    NtSyscallEntry {
        number: 0x00A4,
        name: "NtConnectPort",
    },
    NtSyscallEntry {
        number: 0x0043,
        name: "NtContinue",
    },
    NtSyscallEntry {
        number: 0x00A5,
        name: "NtContinueEx",
    },
    NtSyscallEntry {
        number: 0x00A6,
        name: "NtConvertBetweenAuxiliaryCounterAndPerformanceCounter",
    },
    NtSyscallEntry {
        number: 0x00A7,
        name: "NtCopyFileChunk",
    },
    NtSyscallEntry {
        number: 0x00A8,
        name: "NtCreateCpuPartition",
    },
    NtSyscallEntry {
        number: 0x00A9,
        name: "NtCreateCrossVmEvent",
    },
    NtSyscallEntry {
        number: 0x00AA,
        name: "NtCreateCrossVmMutant",
    },
    NtSyscallEntry {
        number: 0x00AB,
        name: "NtCreateDebugObject",
    },
    NtSyscallEntry {
        number: 0x00AC,
        name: "NtCreateDirectoryObject",
    },
    NtSyscallEntry {
        number: 0x00AD,
        name: "NtCreateDirectoryObjectEx",
    },
    NtSyscallEntry {
        number: 0x00AE,
        name: "NtCreateEnclave",
    },
    NtSyscallEntry {
        number: 0x00AF,
        name: "NtCreateEnlistment",
    },
    NtSyscallEntry {
        number: 0x0048,
        name: "NtCreateEvent",
    },
    NtSyscallEntry {
        number: 0x00B0,
        name: "NtCreateEventPair",
    },
    NtSyscallEntry {
        number: 0x0055,
        name: "NtCreateFile",
    },
    NtSyscallEntry {
        number: 0x00B1,
        name: "NtCreateIRTimer",
    },
    NtSyscallEntry {
        number: 0x00B2,
        name: "NtCreateIoCompletion",
    },
    NtSyscallEntry {
        number: 0x00B3,
        name: "NtCreateIoRing",
    },
    NtSyscallEntry {
        number: 0x00B4,
        name: "NtCreateJobObject",
    },
    NtSyscallEntry {
        number: 0x00B5,
        name: "NtCreateJobSet",
    },
    NtSyscallEntry {
        number: 0x001D,
        name: "NtCreateKey",
    },
    NtSyscallEntry {
        number: 0x00B6,
        name: "NtCreateKeyTransacted",
    },
    NtSyscallEntry {
        number: 0x00B7,
        name: "NtCreateKeyedEvent",
    },
    NtSyscallEntry {
        number: 0x00B8,
        name: "NtCreateLowBoxToken",
    },
    NtSyscallEntry {
        number: 0x00B9,
        name: "NtCreateMailslotFile",
    },
    NtSyscallEntry {
        number: 0x00BA,
        name: "NtCreateMutant",
    },
    NtSyscallEntry {
        number: 0x00BB,
        name: "NtCreateNamedPipeFile",
    },
    NtSyscallEntry {
        number: 0x00BC,
        name: "NtCreatePagingFile",
    },
    NtSyscallEntry {
        number: 0x00BD,
        name: "NtCreatePartition",
    },
    NtSyscallEntry {
        number: 0x00BE,
        name: "NtCreatePort",
    },
    NtSyscallEntry {
        number: 0x00BF,
        name: "NtCreatePrivateNamespace",
    },
    NtSyscallEntry {
        number: 0x00C0,
        name: "NtCreateProcess",
    },
    NtSyscallEntry {
        number: 0x004D,
        name: "NtCreateProcessEx",
    },
    NtSyscallEntry {
        number: 0x00C1,
        name: "NtCreateProcessStateChange",
    },
    NtSyscallEntry {
        number: 0x00C2,
        name: "NtCreateProfile",
    },
    NtSyscallEntry {
        number: 0x00C3,
        name: "NtCreateProfileEx",
    },
    NtSyscallEntry {
        number: 0x00C4,
        name: "NtCreateRegistryTransaction",
    },
    NtSyscallEntry {
        number: 0x00C5,
        name: "NtCreateResourceManager",
    },
    NtSyscallEntry {
        number: 0x004A,
        name: "NtCreateSection",
    },
    NtSyscallEntry {
        number: 0x00C6,
        name: "NtCreateSectionEx",
    },
    NtSyscallEntry {
        number: 0x00C7,
        name: "NtCreateSemaphore",
    },
    NtSyscallEntry {
        number: 0x00C8,
        name: "NtCreateSymbolicLinkObject",
    },
    NtSyscallEntry {
        number: 0x004E,
        name: "NtCreateThread",
    },
    NtSyscallEntry {
        number: 0x00C9,
        name: "NtCreateThreadEx",
    },
    NtSyscallEntry {
        number: 0x00CA,
        name: "NtCreateThreadStateChange",
    },
    NtSyscallEntry {
        number: 0x00CB,
        name: "NtCreateTimer",
    },
    NtSyscallEntry {
        number: 0x00CC,
        name: "NtCreateTimer2",
    },
    NtSyscallEntry {
        number: 0x00CD,
        name: "NtCreateToken",
    },
    NtSyscallEntry {
        number: 0x00CE,
        name: "NtCreateTokenEx",
    },
    NtSyscallEntry {
        number: 0x00CF,
        name: "NtCreateTransaction",
    },
    NtSyscallEntry {
        number: 0x00D0,
        name: "NtCreateTransactionManager",
    },
    NtSyscallEntry {
        number: 0x00D1,
        name: "NtCreateUserProcess",
    },
    NtSyscallEntry {
        number: 0x00D2,
        name: "NtCreateWaitCompletionPacket",
    },
    NtSyscallEntry {
        number: 0x00D3,
        name: "NtCreateWaitablePort",
    },
    NtSyscallEntry {
        number: 0x00D4,
        name: "NtCreateWnfStateName",
    },
    NtSyscallEntry {
        number: 0x00D5,
        name: "NtCreateWorkerFactory",
    },
    NtSyscallEntry {
        number: 0x00D6,
        name: "NtDebugActiveProcess",
    },
    NtSyscallEntry {
        number: 0x00D7,
        name: "NtDebugContinue",
    },
    NtSyscallEntry {
        number: 0x0034,
        name: "NtDelayExecution",
    },
    NtSyscallEntry {
        number: 0x00D8,
        name: "NtDeleteAtom",
    },
    NtSyscallEntry {
        number: 0x00D9,
        name: "NtDeleteBootEntry",
    },
    NtSyscallEntry {
        number: 0x00DA,
        name: "NtDeleteDriverEntry",
    },
    NtSyscallEntry {
        number: 0x00DB,
        name: "NtDeleteFile",
    },
    NtSyscallEntry {
        number: 0x00DC,
        name: "NtDeleteKey",
    },
    NtSyscallEntry {
        number: 0x00DD,
        name: "NtDeleteObjectAuditAlarm",
    },
    NtSyscallEntry {
        number: 0x00DE,
        name: "NtDeletePrivateNamespace",
    },
    NtSyscallEntry {
        number: 0x00DF,
        name: "NtDeleteValueKey",
    },
    NtSyscallEntry {
        number: 0x00E0,
        name: "NtDeleteWnfStateData",
    },
    NtSyscallEntry {
        number: 0x00E1,
        name: "NtDeleteWnfStateName",
    },
    NtSyscallEntry {
        number: 0x0007,
        name: "NtDeviceIoControlFile",
    },
    NtSyscallEntry {
        number: 0x00E2,
        name: "NtDirectGraphicsCall",
    },
    NtSyscallEntry {
        number: 0x00E3,
        name: "NtDisableLastKnownGood",
    },
    NtSyscallEntry {
        number: 0x00E4,
        name: "NtDisplayString",
    },
    NtSyscallEntry {
        number: 0x00E5,
        name: "NtDrawText",
    },
    NtSyscallEntry {
        number: 0x003C,
        name: "NtDuplicateObject",
    },
    NtSyscallEntry {
        number: 0x0042,
        name: "NtDuplicateToken",
    },
    NtSyscallEntry {
        number: 0x00E6,
        name: "NtEnableLastKnownGood",
    },
    NtSyscallEntry {
        number: 0x00E7,
        name: "NtEnumerateBootEntries",
    },
    NtSyscallEntry {
        number: 0x00E8,
        name: "NtEnumerateDriverEntries",
    },
    NtSyscallEntry {
        number: 0x0032,
        name: "NtEnumerateKey",
    },
    NtSyscallEntry {
        number: 0x00E9,
        name: "NtEnumerateSystemEnvironmentValuesEx",
    },
    NtSyscallEntry {
        number: 0x00EA,
        name: "NtEnumerateTransactionObject",
    },
    NtSyscallEntry {
        number: 0x0013,
        name: "NtEnumerateValueKey",
    },
    NtSyscallEntry {
        number: 0x00EB,
        name: "NtExtendSection",
    },
    NtSyscallEntry {
        number: 0x00EC,
        name: "NtFilterBootOption",
    },
    NtSyscallEntry {
        number: 0x00ED,
        name: "NtFilterToken",
    },
    NtSyscallEntry {
        number: 0x00EE,
        name: "NtFilterTokenEx",
    },
    NtSyscallEntry {
        number: 0x0014,
        name: "NtFindAtom",
    },
    NtSyscallEntry {
        number: 0x004B,
        name: "NtFlushBuffersFile",
    },
    NtSyscallEntry {
        number: 0x00EF,
        name: "NtFlushBuffersFileEx",
    },
    NtSyscallEntry {
        number: 0x00F0,
        name: "NtFlushInstallUILanguage",
    },
    NtSyscallEntry {
        number: 0x00F1,
        name: "NtFlushInstructionCache",
    },
    NtSyscallEntry {
        number: 0x00F2,
        name: "NtFlushKey",
    },
    NtSyscallEntry {
        number: 0x00F3,
        name: "NtFlushProcessWriteBuffers",
    },
    NtSyscallEntry {
        number: 0x00F4,
        name: "NtFlushVirtualMemory",
    },
    NtSyscallEntry {
        number: 0x00F5,
        name: "NtFlushWriteBuffer",
    },
    NtSyscallEntry {
        number: 0x00F6,
        name: "NtFreeUserPhysicalPages",
    },
    NtSyscallEntry {
        number: 0x001E,
        name: "NtFreeVirtualMemory",
    },
    NtSyscallEntry {
        number: 0x00F7,
        name: "NtFreezeRegistry",
    },
    NtSyscallEntry {
        number: 0x00F8,
        name: "NtFreezeTransactions",
    },
    NtSyscallEntry {
        number: 0x0039,
        name: "NtFsControlFile",
    },
    NtSyscallEntry {
        number: 0x00F9,
        name: "NtGetCachedSigningLevel",
    },
    NtSyscallEntry {
        number: 0x00FA,
        name: "NtGetCompleteWnfStateSubscription",
    },
    NtSyscallEntry {
        number: 0x00FB,
        name: "NtGetContextThread",
    },
    NtSyscallEntry {
        number: 0x00FC,
        name: "NtGetCurrentProcessorNumber",
    },
    NtSyscallEntry {
        number: 0x00FD,
        name: "NtGetCurrentProcessorNumberEx",
    },
    NtSyscallEntry {
        number: 0x00FE,
        name: "NtGetDevicePowerState",
    },
    NtSyscallEntry {
        number: 0x00FF,
        name: "NtGetMUIRegistryInfo",
    },
    NtSyscallEntry {
        number: 0x0100,
        name: "NtGetNextProcess",
    },
    NtSyscallEntry {
        number: 0x0101,
        name: "NtGetNextThread",
    },
    NtSyscallEntry {
        number: 0x0102,
        name: "NtGetNlsSectionPtr",
    },
    NtSyscallEntry {
        number: 0x0103,
        name: "NtGetNotificationResourceManager",
    },
    NtSyscallEntry {
        number: 0x0104,
        name: "NtGetWriteWatch",
    },
    NtSyscallEntry {
        number: 0x0105,
        name: "NtImpersonateAnonymousToken",
    },
    NtSyscallEntry {
        number: 0x001F,
        name: "NtImpersonateClientOfPort",
    },
    NtSyscallEntry {
        number: 0x0106,
        name: "NtImpersonateThread",
    },
    NtSyscallEntry {
        number: 0x0107,
        name: "NtInitializeEnclave",
    },
    NtSyscallEntry {
        number: 0x0108,
        name: "NtInitializeNlsFiles",
    },
    NtSyscallEntry {
        number: 0x0109,
        name: "NtInitializeRegistry",
    },
    NtSyscallEntry {
        number: 0x010A,
        name: "NtInitiatePowerAction",
    },
    NtSyscallEntry {
        number: 0x004F,
        name: "NtIsProcessInJob",
    },
    NtSyscallEntry {
        number: 0x010B,
        name: "NtIsSystemResumeAutomatic",
    },
    NtSyscallEntry {
        number: 0x010C,
        name: "NtIsUILanguageComitted",
    },
    NtSyscallEntry {
        number: 0x010D,
        name: "NtListenPort",
    },
    NtSyscallEntry {
        number: 0x010E,
        name: "NtLoadDriver",
    },
    NtSyscallEntry {
        number: 0x010F,
        name: "NtLoadEnclaveData",
    },
    NtSyscallEntry {
        number: 0x0110,
        name: "NtLoadKey",
    },
    NtSyscallEntry {
        number: 0x0111,
        name: "NtLoadKey2",
    },
    NtSyscallEntry {
        number: 0x0112,
        name: "NtLoadKey3",
    },
    NtSyscallEntry {
        number: 0x0113,
        name: "NtLoadKeyEx",
    },
    NtSyscallEntry {
        number: 0x0114,
        name: "NtLockFile",
    },
    NtSyscallEntry {
        number: 0x0115,
        name: "NtLockProductActivationKeys",
    },
    NtSyscallEntry {
        number: 0x0116,
        name: "NtLockRegistryKey",
    },
    NtSyscallEntry {
        number: 0x0117,
        name: "NtLockVirtualMemory",
    },
    NtSyscallEntry {
        number: 0x0118,
        name: "NtMakePermanentObject",
    },
    NtSyscallEntry {
        number: 0x0119,
        name: "NtMakeTemporaryObject",
    },
    NtSyscallEntry {
        number: 0x011A,
        name: "NtManageHotPatch",
    },
    NtSyscallEntry {
        number: 0x011B,
        name: "NtManagePartition",
    },
    NtSyscallEntry {
        number: 0x011C,
        name: "NtMapCMFModule",
    },
    NtSyscallEntry {
        number: 0x011D,
        name: "NtMapUserPhysicalPages",
    },
    NtSyscallEntry {
        number: 0x0003,
        name: "NtMapUserPhysicalPagesScatter",
    },
    NtSyscallEntry {
        number: 0x0028,
        name: "NtMapViewOfSection",
    },
    NtSyscallEntry {
        number: 0x011E,
        name: "NtMapViewOfSectionEx",
    },
    NtSyscallEntry {
        number: 0x011F,
        name: "NtModifyBootEntry",
    },
    NtSyscallEntry {
        number: 0x0120,
        name: "NtModifyDriverEntry",
    },
    NtSyscallEntry {
        number: 0x0121,
        name: "NtNotifyChangeDirectoryFile",
    },
    NtSyscallEntry {
        number: 0x0122,
        name: "NtNotifyChangeDirectoryFileEx",
    },
    NtSyscallEntry {
        number: 0x0123,
        name: "NtNotifyChangeKey",
    },
    NtSyscallEntry {
        number: 0x0124,
        name: "NtNotifyChangeMultipleKeys",
    },
    NtSyscallEntry {
        number: 0x0125,
        name: "NtNotifyChangeSession",
    },
    NtSyscallEntry {
        number: 0x0126,
        name: "NtOpenCpuPartition",
    },
    NtSyscallEntry {
        number: 0x0058,
        name: "NtOpenDirectoryObject",
    },
    NtSyscallEntry {
        number: 0x0127,
        name: "NtOpenEnlistment",
    },
    NtSyscallEntry {
        number: 0x0040,
        name: "NtOpenEvent",
    },
    NtSyscallEntry {
        number: 0x0128,
        name: "NtOpenEventPair",
    },
    NtSyscallEntry {
        number: 0x0033,
        name: "NtOpenFile",
    },
    NtSyscallEntry {
        number: 0x0129,
        name: "NtOpenIoCompletion",
    },
    NtSyscallEntry {
        number: 0x012A,
        name: "NtOpenJobObject",
    },
    NtSyscallEntry {
        number: 0x0012,
        name: "NtOpenKey",
    },
    NtSyscallEntry {
        number: 0x012B,
        name: "NtOpenKeyEx",
    },
    NtSyscallEntry {
        number: 0x012C,
        name: "NtOpenKeyTransacted",
    },
    NtSyscallEntry {
        number: 0x012D,
        name: "NtOpenKeyTransactedEx",
    },
    NtSyscallEntry {
        number: 0x012E,
        name: "NtOpenKeyedEvent",
    },
    NtSyscallEntry {
        number: 0x012F,
        name: "NtOpenMutant",
    },
    NtSyscallEntry {
        number: 0x0130,
        name: "NtOpenObjectAuditAlarm",
    },
    NtSyscallEntry {
        number: 0x0131,
        name: "NtOpenPartition",
    },
    NtSyscallEntry {
        number: 0x0132,
        name: "NtOpenPrivateNamespace",
    },
    NtSyscallEntry {
        number: 0x0026,
        name: "NtOpenProcess",
    },
    NtSyscallEntry {
        number: 0x0133,
        name: "NtOpenProcessToken",
    },
    NtSyscallEntry {
        number: 0x0030,
        name: "NtOpenProcessTokenEx",
    },
    NtSyscallEntry {
        number: 0x0134,
        name: "NtOpenRegistryTransaction",
    },
    NtSyscallEntry {
        number: 0x0135,
        name: "NtOpenResourceManager",
    },
    NtSyscallEntry {
        number: 0x0037,
        name: "NtOpenSection",
    },
    NtSyscallEntry {
        number: 0x0136,
        name: "NtOpenSemaphore",
    },
    NtSyscallEntry {
        number: 0x0137,
        name: "NtOpenSession",
    },
    NtSyscallEntry {
        number: 0x0138,
        name: "NtOpenSymbolicLinkObject",
    },
    NtSyscallEntry {
        number: 0x0139,
        name: "NtOpenThread",
    },
    NtSyscallEntry {
        number: 0x0024,
        name: "NtOpenThreadToken",
    },
    NtSyscallEntry {
        number: 0x002F,
        name: "NtOpenThreadTokenEx",
    },
    NtSyscallEntry {
        number: 0x013A,
        name: "NtOpenTimer",
    },
    NtSyscallEntry {
        number: 0x013B,
        name: "NtOpenTransaction",
    },
    NtSyscallEntry {
        number: 0x013C,
        name: "NtOpenTransactionManager",
    },
    NtSyscallEntry {
        number: 0x013D,
        name: "NtPlugPlayControl",
    },
    NtSyscallEntry {
        number: 0x005F,
        name: "NtPowerInformation",
    },
    NtSyscallEntry {
        number: 0x013E,
        name: "NtPrePrepareComplete",
    },
    NtSyscallEntry {
        number: 0x013F,
        name: "NtPrePrepareEnlistment",
    },
    NtSyscallEntry {
        number: 0x0140,
        name: "NtPrepareComplete",
    },
    NtSyscallEntry {
        number: 0x0141,
        name: "NtPrepareEnlistment",
    },
    NtSyscallEntry {
        number: 0x0142,
        name: "NtPrivilegeCheck",
    },
    NtSyscallEntry {
        number: 0x0143,
        name: "NtPrivilegeObjectAuditAlarm",
    },
    NtSyscallEntry {
        number: 0x0144,
        name: "NtPrivilegedServiceAuditAlarm",
    },
    NtSyscallEntry {
        number: 0x0145,
        name: "NtPropagationComplete",
    },
    NtSyscallEntry {
        number: 0x0146,
        name: "NtPropagationFailed",
    },
    NtSyscallEntry {
        number: 0x0050,
        name: "NtProtectVirtualMemory",
    },
    NtSyscallEntry {
        number: 0x0147,
        name: "NtPssCaptureVaSpaceBulk",
    },
    NtSyscallEntry {
        number: 0x0148,
        name: "NtPulseEvent",
    },
    NtSyscallEntry {
        number: 0x003D,
        name: "NtQueryAttributesFile",
    },
    NtSyscallEntry {
        number: 0x0149,
        name: "NtQueryAuxiliaryCounterFrequency",
    },
    NtSyscallEntry {
        number: 0x014A,
        name: "NtQueryBootEntryOrder",
    },
    NtSyscallEntry {
        number: 0x014B,
        name: "NtQueryBootOptions",
    },
    NtSyscallEntry {
        number: 0x014C,
        name: "NtQueryDebugFilterState",
    },
    NtSyscallEntry {
        number: 0x0015,
        name: "NtQueryDefaultLocale",
    },
    NtSyscallEntry {
        number: 0x0044,
        name: "NtQueryDefaultUILanguage",
    },
    NtSyscallEntry {
        number: 0x0035,
        name: "NtQueryDirectoryFile",
    },
    NtSyscallEntry {
        number: 0x014D,
        name: "NtQueryDirectoryFileEx",
    },
    NtSyscallEntry {
        number: 0x014E,
        name: "NtQueryDirectoryObject",
    },
    NtSyscallEntry {
        number: 0x014F,
        name: "NtQueryDriverEntryOrder",
    },
    NtSyscallEntry {
        number: 0x0150,
        name: "NtQueryEaFile",
    },
    NtSyscallEntry {
        number: 0x0056,
        name: "NtQueryEvent",
    },
    NtSyscallEntry {
        number: 0x0151,
        name: "NtQueryFullAttributesFile",
    },
    NtSyscallEntry {
        number: 0x0152,
        name: "NtQueryInformationAtom",
    },
    NtSyscallEntry {
        number: 0x0153,
        name: "NtQueryInformationByName",
    },
    NtSyscallEntry {
        number: 0x0154,
        name: "NtQueryInformationCpuPartition",
    },
    NtSyscallEntry {
        number: 0x0155,
        name: "NtQueryInformationEnlistment",
    },
    NtSyscallEntry {
        number: 0x0011,
        name: "NtQueryInformationFile",
    },
    NtSyscallEntry {
        number: 0x0156,
        name: "NtQueryInformationJobObject",
    },
    NtSyscallEntry {
        number: 0x0157,
        name: "NtQueryInformationPort",
    },
    NtSyscallEntry {
        number: 0x0019,
        name: "NtQueryInformationProcess",
    },
    NtSyscallEntry {
        number: 0x0158,
        name: "NtQueryInformationResourceManager",
    },
    NtSyscallEntry {
        number: 0x0025,
        name: "NtQueryInformationThread",
    },
    NtSyscallEntry {
        number: 0x0021,
        name: "NtQueryInformationToken",
    },
    NtSyscallEntry {
        number: 0x0159,
        name: "NtQueryInformationTransaction",
    },
    NtSyscallEntry {
        number: 0x015A,
        name: "NtQueryInformationTransactionManager",
    },
    NtSyscallEntry {
        number: 0x015B,
        name: "NtQueryInformationWorkerFactory",
    },
    NtSyscallEntry {
        number: 0x015C,
        name: "NtQueryInstallUILanguage",
    },
    NtSyscallEntry {
        number: 0x015D,
        name: "NtQueryIntervalProfile",
    },
    NtSyscallEntry {
        number: 0x015E,
        name: "NtQueryIoCompletion",
    },
    NtSyscallEntry {
        number: 0x015F,
        name: "NtQueryIoRingCapabilities",
    },
    NtSyscallEntry {
        number: 0x0016,
        name: "NtQueryKey",
    },
    NtSyscallEntry {
        number: 0x0160,
        name: "NtQueryLicenseValue",
    },
    NtSyscallEntry {
        number: 0x0161,
        name: "NtQueryMultipleValueKey",
    },
    NtSyscallEntry {
        number: 0x0162,
        name: "NtQueryMutant",
    },
    NtSyscallEntry {
        number: 0x0010,
        name: "NtQueryObject",
    },
    NtSyscallEntry {
        number: 0x0163,
        name: "NtQueryOpenSubKeys",
    },
    NtSyscallEntry {
        number: 0x0164,
        name: "NtQueryOpenSubKeysEx",
    },
    NtSyscallEntry {
        number: 0x0031,
        name: "NtQueryPerformanceCounter",
    },
    NtSyscallEntry {
        number: 0x0165,
        name: "NtQueryPortInformationProcess",
    },
    NtSyscallEntry {
        number: 0x0166,
        name: "NtQueryQuotaInformationFile",
    },
    NtSyscallEntry {
        number: 0x0051,
        name: "NtQuerySection",
    },
    NtSyscallEntry {
        number: 0x0167,
        name: "NtQuerySecurityAttributesToken",
    },
    NtSyscallEntry {
        number: 0x0168,
        name: "NtQuerySecurityObject",
    },
    NtSyscallEntry {
        number: 0x0169,
        name: "NtQuerySecurityPolicy",
    },
    NtSyscallEntry {
        number: 0x016A,
        name: "NtQuerySemaphore",
    },
    NtSyscallEntry {
        number: 0x016B,
        name: "NtQuerySymbolicLinkObject",
    },
    NtSyscallEntry {
        number: 0x016C,
        name: "NtQuerySystemEnvironmentValue",
    },
    NtSyscallEntry {
        number: 0x016D,
        name: "NtQuerySystemEnvironmentValueEx",
    },
    NtSyscallEntry {
        number: 0x0036,
        name: "NtQuerySystemInformation",
    },
    NtSyscallEntry {
        number: 0x016E,
        name: "NtQuerySystemInformationEx",
    },
    NtSyscallEntry {
        number: 0x005A,
        name: "NtQuerySystemTime",
    },
    NtSyscallEntry {
        number: 0x0038,
        name: "NtQueryTimer",
    },
    NtSyscallEntry {
        number: 0x016F,
        name: "NtQueryTimerResolution",
    },
    NtSyscallEntry {
        number: 0x0017,
        name: "NtQueryValueKey",
    },
    NtSyscallEntry {
        number: 0x0023,
        name: "NtQueryVirtualMemory",
    },
    NtSyscallEntry {
        number: 0x0049,
        name: "NtQueryVolumeInformationFile",
    },
    NtSyscallEntry {
        number: 0x0170,
        name: "NtQueryWnfStateData",
    },
    NtSyscallEntry {
        number: 0x0171,
        name: "NtQueryWnfStateNameInformation",
    },
    NtSyscallEntry {
        number: 0x0045,
        name: "NtQueueApcThread",
    },
    NtSyscallEntry {
        number: 0x0172,
        name: "NtQueueApcThreadEx",
    },
    NtSyscallEntry {
        number: 0x0173,
        name: "NtQueueApcThreadEx2",
    },
    NtSyscallEntry {
        number: 0x0174,
        name: "NtRaiseException",
    },
    NtSyscallEntry {
        number: 0x0175,
        name: "NtRaiseHardError",
    },
    NtSyscallEntry {
        number: 0x0006,
        name: "NtReadFile",
    },
    NtSyscallEntry {
        number: 0x002E,
        name: "NtReadFileScatter",
    },
    NtSyscallEntry {
        number: 0x0176,
        name: "NtReadOnlyEnlistment",
    },
    NtSyscallEntry {
        number: 0x0054,
        name: "NtReadRequestData",
    },
    NtSyscallEntry {
        number: 0x003F,
        name: "NtReadVirtualMemory",
    },
    NtSyscallEntry {
        number: 0x0177,
        name: "NtReadVirtualMemoryEx",
    },
    NtSyscallEntry {
        number: 0x0178,
        name: "NtRecoverEnlistment",
    },
    NtSyscallEntry {
        number: 0x0179,
        name: "NtRecoverResourceManager",
    },
    NtSyscallEntry {
        number: 0x017A,
        name: "NtRecoverTransactionManager",
    },
    NtSyscallEntry {
        number: 0x017B,
        name: "NtRegisterProtocolAddressInformation",
    },
    NtSyscallEntry {
        number: 0x017C,
        name: "NtRegisterThreadTerminatePort",
    },
    NtSyscallEntry {
        number: 0x017D,
        name: "NtReleaseKeyedEvent",
    },
    NtSyscallEntry {
        number: 0x0020,
        name: "NtReleaseMutant",
    },
    NtSyscallEntry {
        number: 0x000A,
        name: "NtReleaseSemaphore",
    },
    NtSyscallEntry {
        number: 0x017E,
        name: "NtReleaseWorkerFactoryWorker",
    },
    NtSyscallEntry {
        number: 0x0009,
        name: "NtRemoveIoCompletion",
    },
    NtSyscallEntry {
        number: 0x017F,
        name: "NtRemoveIoCompletionEx",
    },
    NtSyscallEntry {
        number: 0x0180,
        name: "NtRemoveProcessDebug",
    },
    NtSyscallEntry {
        number: 0x0181,
        name: "NtRenameKey",
    },
    NtSyscallEntry {
        number: 0x0182,
        name: "NtRenameTransactionManager",
    },
    NtSyscallEntry {
        number: 0x0183,
        name: "NtReplaceKey",
    },
    NtSyscallEntry {
        number: 0x0184,
        name: "NtReplacePartitionUnit",
    },
    NtSyscallEntry {
        number: 0x000C,
        name: "NtReplyPort",
    },
    NtSyscallEntry {
        number: 0x000B,
        name: "NtReplyWaitReceivePort",
    },
    NtSyscallEntry {
        number: 0x002B,
        name: "NtReplyWaitReceivePortEx",
    },
    NtSyscallEntry {
        number: 0x0185,
        name: "NtReplyWaitReplyPort",
    },
    NtSyscallEntry {
        number: 0x0186,
        name: "NtRequestPort",
    },
    NtSyscallEntry {
        number: 0x0022,
        name: "NtRequestWaitReplyPort",
    },
    NtSyscallEntry {
        number: 0x0187,
        name: "NtResetEvent",
    },
    NtSyscallEntry {
        number: 0x0188,
        name: "NtResetWriteWatch",
    },
    NtSyscallEntry {
        number: 0x0189,
        name: "NtRestoreKey",
    },
    NtSyscallEntry {
        number: 0x018A,
        name: "NtResumeProcess",
    },
    NtSyscallEntry {
        number: 0x0052,
        name: "NtResumeThread",
    },
    NtSyscallEntry {
        number: 0x018B,
        name: "NtRevertContainerImpersonation",
    },
    NtSyscallEntry {
        number: 0x018C,
        name: "NtRollbackComplete",
    },
    NtSyscallEntry {
        number: 0x018D,
        name: "NtRollbackEnlistment",
    },
    NtSyscallEntry {
        number: 0x018E,
        name: "NtRollbackRegistryTransaction",
    },
    NtSyscallEntry {
        number: 0x018F,
        name: "NtRollbackTransaction",
    },
    NtSyscallEntry {
        number: 0x0190,
        name: "NtRollforwardTransactionManager",
    },
    NtSyscallEntry {
        number: 0x0191,
        name: "NtSaveKey",
    },
    NtSyscallEntry {
        number: 0x0192,
        name: "NtSaveKeyEx",
    },
    NtSyscallEntry {
        number: 0x0193,
        name: "NtSaveMergedKeys",
    },
    NtSyscallEntry {
        number: 0x0194,
        name: "NtSecureConnectPort",
    },
    NtSyscallEntry {
        number: 0x0195,
        name: "NtSerializeBoot",
    },
    NtSyscallEntry {
        number: 0x0196,
        name: "NtSetBootEntryOrder",
    },
    NtSyscallEntry {
        number: 0x0197,
        name: "NtSetBootOptions",
    },
    NtSyscallEntry {
        number: 0x0198,
        name: "NtSetCachedSigningLevel",
    },
    NtSyscallEntry {
        number: 0x0199,
        name: "NtSetCachedSigningLevel2",
    },
    NtSyscallEntry {
        number: 0x019A,
        name: "NtSetContextThread",
    },
    NtSyscallEntry {
        number: 0x019B,
        name: "NtSetDebugFilterState",
    },
    NtSyscallEntry {
        number: 0x019C,
        name: "NtSetDefaultHardErrorPort",
    },
    NtSyscallEntry {
        number: 0x019D,
        name: "NtSetDefaultLocale",
    },
    NtSyscallEntry {
        number: 0x019E,
        name: "NtSetDefaultUILanguage",
    },
    NtSyscallEntry {
        number: 0x019F,
        name: "NtSetDriverEntryOrder",
    },
    NtSyscallEntry {
        number: 0x01A0,
        name: "NtSetEaFile",
    },
    NtSyscallEntry {
        number: 0x000E,
        name: "NtSetEvent",
    },
    NtSyscallEntry {
        number: 0x002D,
        name: "NtSetEventBoostPriority",
    },
    NtSyscallEntry {
        number: 0x01A1,
        name: "NtSetEventEx",
    },
    NtSyscallEntry {
        number: 0x01A2,
        name: "NtSetHighEventPair",
    },
    NtSyscallEntry {
        number: 0x01A3,
        name: "NtSetHighWaitLowEventPair",
    },
    NtSyscallEntry {
        number: 0x01A4,
        name: "NtSetIRTimer",
    },
    NtSyscallEntry {
        number: 0x01A5,
        name: "NtSetInformationCpuPartition",
    },
    NtSyscallEntry {
        number: 0x01A6,
        name: "NtSetInformationDebugObject",
    },
    NtSyscallEntry {
        number: 0x01A7,
        name: "NtSetInformationEnlistment",
    },
    NtSyscallEntry {
        number: 0x0027,
        name: "NtSetInformationFile",
    },
    NtSyscallEntry {
        number: 0x01A8,
        name: "NtSetInformationIoRing",
    },
    NtSyscallEntry {
        number: 0x01A9,
        name: "NtSetInformationJobObject",
    },
    NtSyscallEntry {
        number: 0x01AA,
        name: "NtSetInformationKey",
    },
    NtSyscallEntry {
        number: 0x005C,
        name: "NtSetInformationObject",
    },
    NtSyscallEntry {
        number: 0x001C,
        name: "NtSetInformationProcess",
    },
    NtSyscallEntry {
        number: 0x01AB,
        name: "NtSetInformationResourceManager",
    },
    NtSyscallEntry {
        number: 0x01AC,
        name: "NtSetInformationSymbolicLink",
    },
    NtSyscallEntry {
        number: 0x000D,
        name: "NtSetInformationThread",
    },
    NtSyscallEntry {
        number: 0x01AD,
        name: "NtSetInformationToken",
    },
    NtSyscallEntry {
        number: 0x01AE,
        name: "NtSetInformationTransaction",
    },
    NtSyscallEntry {
        number: 0x01AF,
        name: "NtSetInformationTransactionManager",
    },
    NtSyscallEntry {
        number: 0x01B0,
        name: "NtSetInformationVirtualMemory",
    },
    NtSyscallEntry {
        number: 0x01B1,
        name: "NtSetInformationWorkerFactory",
    },
    NtSyscallEntry {
        number: 0x01B2,
        name: "NtSetIntervalProfile",
    },
    NtSyscallEntry {
        number: 0x01B3,
        name: "NtSetIoCompletion",
    },
    NtSyscallEntry {
        number: 0x01B4,
        name: "NtSetIoCompletionEx",
    },
    NtSyscallEntry {
        number: 0x01B5,
        name: "NtSetLdtEntries",
    },
    NtSyscallEntry {
        number: 0x01B6,
        name: "NtSetLowEventPair",
    },
    NtSyscallEntry {
        number: 0x01B7,
        name: "NtSetLowWaitHighEventPair",
    },
    NtSyscallEntry {
        number: 0x01B8,
        name: "NtSetQuotaInformationFile",
    },
    NtSyscallEntry {
        number: 0x01B9,
        name: "NtSetSecurityObject",
    },
    NtSyscallEntry {
        number: 0x01BA,
        name: "NtSetSystemEnvironmentValue",
    },
    NtSyscallEntry {
        number: 0x01BB,
        name: "NtSetSystemEnvironmentValueEx",
    },
    NtSyscallEntry {
        number: 0x01BC,
        name: "NtSetSystemInformation",
    },
    NtSyscallEntry {
        number: 0x01BD,
        name: "NtSetSystemPowerState",
    },
    NtSyscallEntry {
        number: 0x01BE,
        name: "NtSetSystemTime",
    },
    NtSyscallEntry {
        number: 0x01BF,
        name: "NtSetThreadExecutionState",
    },
    NtSyscallEntry {
        number: 0x0062,
        name: "NtSetTimer",
    },
    NtSyscallEntry {
        number: 0x01C0,
        name: "NtSetTimer2",
    },
    NtSyscallEntry {
        number: 0x01C1,
        name: "NtSetTimerEx",
    },
    NtSyscallEntry {
        number: 0x01C2,
        name: "NtSetTimerResolution",
    },
    NtSyscallEntry {
        number: 0x01C3,
        name: "NtSetUuidSeed",
    },
    NtSyscallEntry {
        number: 0x0060,
        name: "NtSetValueKey",
    },
    NtSyscallEntry {
        number: 0x01C4,
        name: "NtSetVolumeInformationFile",
    },
    NtSyscallEntry {
        number: 0x01C5,
        name: "NtSetWnfProcessNotificationEvent",
    },
    NtSyscallEntry {
        number: 0x01C6,
        name: "NtShutdownSystem",
    },
    NtSyscallEntry {
        number: 0x01C7,
        name: "NtShutdownWorkerFactory",
    },
    NtSyscallEntry {
        number: 0x01C8,
        name: "NtSignalAndWaitForSingleObject",
    },
    NtSyscallEntry {
        number: 0x01C9,
        name: "NtSinglePhaseReject",
    },
    NtSyscallEntry {
        number: 0x01CA,
        name: "NtStartProfile",
    },
    NtSyscallEntry {
        number: 0x01CB,
        name: "NtStopProfile",
    },
    NtSyscallEntry {
        number: 0x01CC,
        name: "NtSubmitIoRing",
    },
    NtSyscallEntry {
        number: 0x01CD,
        name: "NtSubscribeWnfStateChange",
    },
    NtSyscallEntry {
        number: 0x01CE,
        name: "NtSuspendProcess",
    },
    NtSyscallEntry {
        number: 0x01CF,
        name: "NtSuspendThread",
    },
    NtSyscallEntry {
        number: 0x01D0,
        name: "NtSystemDebugControl",
    },
    NtSyscallEntry {
        number: 0x01D1,
        name: "NtTerminateEnclave",
    },
    NtSyscallEntry {
        number: 0x01D2,
        name: "NtTerminateJobObject",
    },
    NtSyscallEntry {
        number: 0x002C,
        name: "NtTerminateProcess",
    },
    NtSyscallEntry {
        number: 0x0053,
        name: "NtTerminateThread",
    },
    NtSyscallEntry {
        number: 0x01D3,
        name: "NtTestAlert",
    },
    NtSyscallEntry {
        number: 0x01D4,
        name: "NtThawRegistry",
    },
    NtSyscallEntry {
        number: 0x01D5,
        name: "NtThawTransactions",
    },
    NtSyscallEntry {
        number: 0x01D6,
        name: "NtTraceControl",
    },
    NtSyscallEntry {
        number: 0x005E,
        name: "NtTraceEvent",
    },
    NtSyscallEntry {
        number: 0x01D7,
        name: "NtTranslateFilePath",
    },
    NtSyscallEntry {
        number: 0x01D8,
        name: "NtUmsThreadYield",
    },
    NtSyscallEntry {
        number: 0x01D9,
        name: "NtUnloadDriver",
    },
    NtSyscallEntry {
        number: 0x01DA,
        name: "NtUnloadKey",
    },
    NtSyscallEntry {
        number: 0x01DB,
        name: "NtUnloadKey2",
    },
    NtSyscallEntry {
        number: 0x01DC,
        name: "NtUnloadKeyEx",
    },
    NtSyscallEntry {
        number: 0x01DD,
        name: "NtUnlockFile",
    },
    NtSyscallEntry {
        number: 0x01DE,
        name: "NtUnlockVirtualMemory",
    },
    NtSyscallEntry {
        number: 0x002A,
        name: "NtUnmapViewOfSection",
    },
    NtSyscallEntry {
        number: 0x01DF,
        name: "NtUnmapViewOfSectionEx",
    },
    NtSyscallEntry {
        number: 0x01E0,
        name: "NtUnsubscribeWnfStateChange",
    },
    NtSyscallEntry {
        number: 0x01E1,
        name: "NtUpdateWnfStateData",
    },
    NtSyscallEntry {
        number: 0x01E2,
        name: "NtVdmControl",
    },
    NtSyscallEntry {
        number: 0x01E3,
        name: "NtWaitForAlertByThreadId",
    },
    NtSyscallEntry {
        number: 0x01E4,
        name: "NtWaitForDebugEvent",
    },
    NtSyscallEntry {
        number: 0x01E5,
        name: "NtWaitForKeyedEvent",
    },
    NtSyscallEntry {
        number: 0x005B,
        name: "NtWaitForMultipleObjects",
    },
    NtSyscallEntry {
        number: 0x001A,
        name: "NtWaitForMultipleObjects32",
    },
    NtSyscallEntry {
        number: 0x0004,
        name: "NtWaitForSingleObject",
    },
    NtSyscallEntry {
        number: 0x01E6,
        name: "NtWaitForWorkViaWorkerFactory",
    },
    NtSyscallEntry {
        number: 0x01E7,
        name: "NtWaitHighEventPair",
    },
    NtSyscallEntry {
        number: 0x01E8,
        name: "NtWaitLowEventPair",
    },
    NtSyscallEntry {
        number: 0x0001,
        name: "NtWorkerFactoryWorkerReady",
    },
    NtSyscallEntry {
        number: 0x0008,
        name: "NtWriteFile",
    },
    NtSyscallEntry {
        number: 0x001B,
        name: "NtWriteFileGather",
    },
    NtSyscallEntry {
        number: 0x0057,
        name: "NtWriteRequestData",
    },
    NtSyscallEntry {
        number: 0x003A,
        name: "NtWriteVirtualMemory",
    },
    NtSyscallEntry {
        number: 0x0046,
        name: "NtYieldExecution",
    },
];

/// Look up syscall number by name
pub fn lookup_by_name(name: &str) -> Option<u16> {
    let mut left = 0usize;
    let mut right = _NT_SYSCALLS_BY_NAME.len();
    while left < right {
        let mid = left + ((right - left) / 2);
        match _NT_SYSCALLS_BY_NAME[mid].name.cmp(name) {
            core::cmp::Ordering::Less => left = mid + 1,
            core::cmp::Ordering::Greater => right = mid,
            core::cmp::Ordering::Equal => return Some(_NT_SYSCALLS_BY_NAME[mid].number),
        }
    }
    None
}

/// Look up syscall name by number
pub fn lookup_by_number(number: u16) -> Option<&'static str> {
    let mut left = 0usize;
    let mut right = NT_SYSCALL_TABLE.len();
    while left < right {
        let mid = left + ((right - left) / 2);
        let value = NT_SYSCALL_TABLE[mid].number;
        if value < number {
            left = mid + 1;
        } else if value > number {
            right = mid;
        } else {
            return Some(NT_SYSCALL_TABLE[mid].name);
        }
    }
    None
}
