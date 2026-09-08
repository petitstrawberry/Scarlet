# Native syscall boundary checklist for the 32-bit foundation

Snapshot of `kernel/src/syscall/mod.rs` at
`7f591dd5898e09d137ba97b0a591d6f09f97dec8` (2026-09-08).

This checklist accounts for all 143 dispatch entries, including `Invalid` and
5 compatibility-only event entries absent from the public `scarlet-abi::Syscall`
enum. The other 138 entries have a public numeric definition. A review family
assigns the common boundary work; it does not certify a handler as portable or
change its ABI. Optional/debug handlers may have feature-disabled stubs.

See the [audit and design sequence](32bit-foundation-audit.md) for finding IDs,
priorities, evidence and test requirements. This is a foundation checklist,
not an ARMv5TE/RV32 port plan.

## Boundary families

| Family | Review obligation | Findings |
| --- | --- | --- |
| INVALID | Invalid dispatch entry; preserve rejection behavior. | ABI-12 |
| TASK | Process/thread IDs, signed selectors/status and lifecycle continuation; bound kernel IDs at the published ABI. | ABI-02, ABI-11, ABI-12 |
| CLONE | Clone/namespace flags and request decoding; separate typed lifecycle operations from register placement. | ABI-01, ABI-02 |
| EXEC | User pointer arrays, ABI identity, binary loading and execution replacement. | ABI-10, ELF-02, ELF-03 |
| VM | Address/length arithmetic, layout policy and pointer bounds; mmap additionally needs a wide object offset. | ABI-07, ABI-10, MEM-01, MEM-02 |
| SCALAR | Small scalar/control operations; preserve each status convention and architecture hook. | ABI-02, ABI-12, OPT-01 |
| TIME | Full-width nanoseconds through word-sized transport; clock errors/sentinels are separate from values. | ABI-05 |
| RANDOM | Validated writable byte range, count and flags; random-state synchronization provider. | ABI-10, SYN-05 |
| SNAPSHOT | Snapshot field widths, padding, record strides, counts and counter publication. | ABI-04, ABI-11, SYN-04 |
| TLS | TLS/TID pointers, cleanup metadata and architecture-specific thread state. | ABI-10, OPT-03 |
| SCHED | Signed nice values, CPU masks and deadline/state records; validate encoded wide pointers before narrowing. | ABI-08, SYN-03, SYN-06 |
| FUTEX | 32-bit shared word access and synchronization contract; wide timeout and sentinel encoding. | ABI-09, SYN-06 |
| KERNEL_INFO | Fixed byte-string record; retain its 160-byte layout and safe output copying. | ABI-10 |
| HANDLE | Fixed handle identifiers, query records, ownership and operation-specific errors. | ABI-12, SYN-02 |
| CONTROL | Second-level ABI dispatch: classify every device command, nested pointer and control result. | DEV-04, DEV-05, DEV-06 |
| STREAM | Byte-buffer bounds, short counts and signed/error result encodings. | ABI-10, ABI-12 |
| POLL | In-memory wide timeout record, aligned decoding and checked array sizing. | ABI-09 |
| FILE64 | Signed seek and wide file size/position transport, including shared VFS quantities. | ABI-06, MEM-05 |
| FILE_META | Fixed-width metadata encoding plus the internal native-width file-size bottleneck. | MEM-05 |
| FS | Path/user-buffer decoding, flags and filesystem operations through validated common requests. | ABI-10, MEM-05 |
| VIEW | View handles, paths and composition inputs; preserve capability/lifetime semantics. | ABI-10, SYN-02 |
| IPC | Handle transfer, user buffers, event payload and callback/filter records; preserve fixed widths and ownership. | ABI-12, ABI-13, SYN-02 |
| EVENT | Native event registration/mask/frame/restorer and nonordinary return continuation. | ABI-02, ABI-13 |
| NETWORK | Socket arguments/address records and duplicated network configuration/status layouts. | ABI-10, DEV-04 |
| SHV | Optional virtualization service; isolate unsupported backends and retain guest-state widths. | OPT-02 |
| LSM | Module IDs/list buffers, object class and module/toolchain ABI. | ELF-04 |
| ENV | Environment handles/root paths; capability and shared-state lifetime contracts. | ABI-10, SYN-02 |
| ENV_EXEC | RawEnvironmentExec layout, nested argv/envp and handle-transfer arrays. | ABI-03, ELF-02, ELF-03 |

## All current entries

The name is the kernel dispatch name; a different public name is shown in
parentheses. Implementation links point to the primary handler (the feature-on
implementation for SHV), not a claim that all downstream code was exercised.

| Number | Name | Family | Handler |
| ---: | --- | --- | --- |
| 0 | Invalid | INVALID | [`invalid entry`](../../kernel/src/syscall/mod.rs#L196) |
| 1 | Exit | TASK | [`sys_exit`](../../kernel/src/task/syscall.rs#L323) |
| 2 | Clone | CLONE | [`sys_clone`](../../kernel/src/task/syscall.rs#L368) |
| 3 | Execve | EXEC | [`sys_execve`](../../kernel/src/task/syscall.rs#L639) |
| 4 | ExecveABI | EXEC | [`sys_execve_abi`](../../kernel/src/task/syscall.rs#L723) |
| 5 | Waitpid | TASK | [`sys_waitpid`](../../kernel/src/task/syscall.rs#L787) |
| 6 | Kill | TASK | [`sys_kill`](../../kernel/src/task/syscall.rs#L1023) |
| 7 | Getpid | TASK | [`sys_getpid`](../../kernel/src/task/syscall.rs#L967) |
| 8 | Getppid | TASK | [`sys_getppid`](../../kernel/src/task/syscall.rs#L975) |
| 12 | Brk | VM | [`sys_brk`](../../kernel/src/task/syscall.rs#L259) |
| 13 | Sbrk | VM | [`sys_sbrk`](../../kernel/src/task/syscall.rs#L269) |
| 16 | Putchar | SCALAR | [`sys_putchar`](../../kernel/src/task/syscall.rs#L279) |
| 17 | Getchar | SCALAR | [`sys_getchar`](../../kernel/src/task/syscall.rs#L305) |
| 20 | Sleep | TIME | [`sys_sleep`](../../kernel/src/task/syscall.rs#L1598) |
| 21 | Yield | SCALAR | [`sys_yield`](../../kernel/src/task/syscall.rs#L1693) |
| 22 | GetRandom | RANDOM | [`sys_get_random`](../../kernel/src/random.rs#L277) |
| 23 | ExitGroup | TASK | [`sys_exit_group`](../../kernel/src/task/syscall.rs#L1722) |
| 24 | GetTaskInfoCount | SNAPSHOT | [`sys_get_task_info_count`](../../kernel/src/task/syscall.rs#L2040) |
| 25 | GetTaskInfoList | SNAPSHOT | [`sys_get_task_info_list`](../../kernel/src/task/syscall.rs#L2057) |
| 26 | CreateSession | TASK | [`sys_create_session`](../../kernel/src/task/syscall.rs#L1069) |
| 27 | GetSessionId | TASK | [`sys_get_session_id`](../../kernel/src/task/syscall.rs#L1091) |
| 28 | GetProcessGroupId | TASK | [`sys_get_process_group_id`](../../kernel/src/task/syscall.rs#L1124) |
| 29 | SetProcessGroup | TASK | [`sys_set_process_group`](../../kernel/src/task/syscall.rs#L1159) |
| 30 | SetTls | TLS | [`sys_set_tls`](../../kernel/src/task/syscall.rs#L585) |
| 31 | GetTls | TLS | [`sys_get_tls`](../../kernel/src/task/syscall.rs#L606) |
| 32 | SetTidAddress | TLS | [`sys_set_tid_address`](../../kernel/src/task/syscall.rs#L624) |
| 33 | ThreadDetach | TASK | [`sys_thread_detach`](../../kernel/src/task/syscall.rs#L524) |
| 34 | ThreadExitCleanup | TLS | [`sys_thread_exit_cleanup`](../../kernel/src/task/syscall.rs#L348) |
| 35 | MonotonicTime | TIME | [`sys_monotonic_time`](../../kernel/src/task/syscall.rs#L1621) |
| 36 | GetCpuUsageInfo | SNAPSHOT | [`sys_get_cpu_usage_info`](../../kernel/src/task/syscall.rs#L1651) |
| 37 | SystemTime | TIME | [`sys_system_time`](../../kernel/src/task/syscall.rs#L1635) |
| 38 | SetTaskUtilMin | SCHED | [`sys_set_task_util_min`](../../kernel/src/task/syscall.rs#L1220) |
| 39 | GetTaskUtilMin | SCHED | [`sys_get_task_util_min`](../../kernel/src/task/syscall.rs#L1243) |
| 40 | SetTaskNice | SCHED | [`sys_set_task_nice`](../../kernel/src/task/syscall.rs#L1258) |
| 41 | GetTaskNice | SCHED | [`sys_get_task_nice`](../../kernel/src/task/syscall.rs#L1284) |
| 42 | SetTaskCpuAffinity | SCHED | [`sys_set_task_cpu_affinity`](../../kernel/src/task/syscall.rs#L1299) |
| 43 | GetTaskCpuAffinity | SCHED | [`sys_get_task_cpu_affinity`](../../kernel/src/task/syscall.rs#L1327) |
| 44 | SetTaskDeadline | SCHED | [`sys_set_task_deadline`](../../kernel/src/task/syscall.rs#L1345) |
| 45 | GetTaskDeadline | SCHED | [`sys_get_task_deadline`](../../kernel/src/task/syscall.rs#L1382) |
| 46 | SetSchedulerAttr | SCHED | [`sys_set_scheduler_attr`](../../kernel/src/task/syscall.rs#L1411) |
| 47 | GetSchedulerAttr | SCHED | [`sys_get_scheduler_attr`](../../kernel/src/task/syscall.rs#L1442) |
| 48 | GetSchedulerState | SCHED | [`sys_get_scheduler_state`](../../kernel/src/task/syscall.rs#L1498) |
| 49 | FutexWait | FUTEX | [`sys_futex_wait`](../../kernel/src/sync/futex.rs#L107) |
| 50 | FutexWake | FUTEX | [`sys_futex_wake`](../../kernel/src/sync/futex.rs#L150) |
| 51 | GetKernelInfo | KERNEL_INFO | [`sys_get_kernel_info`](../../kernel/src/system.rs#L60) |
| 90 | RegisterAbiZone | VM | [`sys_register_abi_zone`](../../kernel/src/task/syscall.rs#L1740) |
| 91 | UnregisterAbiZone | VM | [`sys_unregister_abi_zone`](../../kernel/src/task/syscall.rs#L1797) |
| 92 | CreateNamespace | CLONE | [`sys_create_namespace`](../../kernel/src/task/syscall.rs#L1844) |
| 100 | HandleQuery | HANDLE | [`sys_handle_query`](../../kernel/src/object/handle/syscall.rs#L26) |
| 101 | HandleSetRole | HANDLE | [`sys_handle_set_role`](../../kernel/src/object/handle/syscall.rs#L67) |
| 102 | HandleClose | HANDLE | [`sys_handle_close`](../../kernel/src/object/handle/syscall.rs#L116) |
| 103 | HandleDuplicate | HANDLE | [`sys_handle_duplicate`](../../kernel/src/object/handle/syscall.rs#L143) |
| 110 | HandleControl | CONTROL | [`sys_handle_control`](../../kernel/src/object/handle/syscall.rs#L203) |
| 200 | StreamRead | STREAM | [`sys_stream_read`](../../kernel/src/object/capability/stream/syscall.rs#L32) |
| 201 | StreamWrite | STREAM | [`sys_stream_write`](../../kernel/src/object/capability/stream/syscall.rs#L85) |
| 202 | Poll | POLL | [`sys_poll`](../../kernel/src/object/capability/selectable/syscall.rs#L88) |
| 300 | FileSeek | FILE64 | [`sys_file_seek`](../../kernel/src/object/capability/file/syscall.rs#L22) |
| 301 | FileTruncate | FILE64 | [`sys_file_truncate`](../../kernel/src/object/capability/file/syscall.rs#L71) |
| 302 | FileMetadata | FILE_META | [`sys_file_metadata`](../../kernel/src/object/capability/file/syscall.rs#L113) |
| 400 | VfsOpen | FS | [`sys_vfs_open`](../../kernel/src/fs/vfs_v2/syscall.rs#L76) |
| 401 | VfsRemove | FS | [`sys_vfs_remove`](../../kernel/src/fs/vfs_v2/syscall.rs#L795) |
| 402 | VfsCreateFile | FS | [`sys_vfs_create_file`](../../kernel/src/fs/vfs_v2/syscall.rs#L327) |
| 403 | VfsCreateDirectory | FS | [`sys_vfs_create_directory`](../../kernel/src/fs/vfs_v2/syscall.rs#L365) |
| 404 | VfsChangeDirectory | FS | [`sys_vfs_change_directory`](../../kernel/src/fs/vfs_v2/syscall.rs#L748) |
| 405 | VfsTruncate | FILE64 | [`sys_vfs_truncate`](../../kernel/src/fs/vfs_v2/syscall.rs#L168) |
| 406 | VfsCreateSymlink | FS | [`sys_vfs_create_symlink`](../../kernel/src/fs/vfs_v2/syscall.rs#L845) |
| 407 | VfsReadlink | FS | [`sys_vfs_readlink`](../../kernel/src/fs/vfs_v2/syscall.rs#L892) |
| 408 | VfsGetCwdPath | FS | [`sys_vfs_get_cwd_path`](../../kernel/src/fs/vfs_v2/syscall.rs#L962) |
| 409 | VfsRename | FS | [`sys_vfs_rename`](../../kernel/src/fs/vfs_v2/syscall.rs#L1008) |
| 410 | VfsMetadata | FILE_META | [`sys_vfs_metadata`](../../kernel/src/fs/vfs_v2/syscall.rs#L214) |
| 411 | VfsCreateHardlink | FS | [`sys_vfs_create_hardlink`](../../kernel/src/fs/vfs_v2/syscall.rs#L280) |
| 412 | VfsSymlinkMetadata | FILE_META | [`sys_vfs_symlink_metadata`](../../kernel/src/fs/vfs_v2/syscall.rs#L222) |
| 500 | FsMount | FS | [`sys_fs_mount`](../../kernel/src/fs/vfs_v2/syscall.rs#L406) |
| 501 | FsUmount | FS | [`sys_fs_umount`](../../kernel/src/fs/vfs_v2/syscall.rs#L533) |
| 502 | FsPivotRoot | FS | [`sys_fs_pivot_root`](../../kernel/src/fs/vfs_v2/syscall.rs#L609) |
| 520 | VfsViewCreate | VIEW | [`sys_vfs_view_create`](../../kernel/src/executor/syscall.rs#L200) |
| 521 | VfsViewCurrent | VIEW | [`sys_vfs_view_current`](../../kernel/src/executor/syscall.rs#L216) |
| 522 | VfsViewClone | VIEW | [`sys_vfs_view_clone`](../../kernel/src/executor/syscall.rs#L224) |
| 523 | VfsViewOpen | VIEW | [`sys_vfs_view_open`](../../kernel/src/executor/syscall.rs#L238) |
| 524 | VfsViewMount | VIEW | [`sys_vfs_view_mount`](../../kernel/src/executor/syscall.rs#L282) |
| 525 | VfsViewBind | VIEW | [`sys_vfs_view_bind`](../../kernel/src/executor/syscall.rs#L302) |
| 526 | VfsViewOverlay | VIEW | [`sys_vfs_view_overlay`](../../kernel/src/executor/syscall.rs#L319) |
| 527 | VfsViewUnmount | VIEW | [`sys_vfs_view_unmount`](../../kernel/src/executor/syscall.rs#L349) |
| 528 | VfsViewCreateDirectory | VIEW | [`sys_vfs_view_mkdir`](../../kernel/src/executor/syscall.rs#L275) |
| 529 | VfsViewRoot | VIEW | [`sys_vfs_view_root`](../../kernel/src/executor/syscall.rs#L230) |
| 600 | Pipe | IPC | [`sys_pipe`](../../kernel/src/ipc/syscall.rs#L45) |
| 610 | EventChannelCreate (compatibility only) | IPC | [`sys_event_channel_create`](../../kernel/src/ipc/syscall.rs#L128) |
| 611 | EventSubscribe (compatibility only) | IPC | [`sys_event_subscribe`](../../kernel/src/ipc/syscall.rs#L156) |
| 612 | EventUnsubscribe (compatibility only) | IPC | [`sys_event_unsubscribe`](../../kernel/src/ipc/syscall.rs#L187) |
| 613 | EventPublish (compatibility only) | IPC | [`sys_event_publish`](../../kernel/src/ipc/syscall.rs#L228) |
| 614 | EventHandlerRegister (compatibility only) | IPC | [`sys_event_handler_register`](../../kernel/src/ipc/syscall.rs#L277) |
| 615 | EventSendDirect | IPC | [`sys_event_send_direct`](../../kernel/src/ipc/syscall.rs#L323) |
| 616 | EventSendGroup | IPC | [`sys_event_send_group`](../../kernel/src/ipc/syscall.rs#L395) |
| 620 | SharedMemoryCreate | VM | [`sys_shared_memory_create`](../../kernel/src/ipc/syscall.rs#L476) |
| 621 | SharedMemoryResize | VM | [`sys_shared_memory_resize`](../../kernel/src/ipc/syscall.rs#L535) |
| 630 | SocketSendHandle | IPC | [`sys_socket_send_handle`](../../kernel/src/ipc/syscall.rs#L600) |
| 631 | SocketRecvHandle | IPC | [`sys_socket_recv_handle`](../../kernel/src/ipc/syscall.rs#L654) |
| 632 | SocketSendHandleAndData | IPC | [`sys_socket_send_handle_and_data`](../../kernel/src/ipc/syscall.rs#L717) |
| 633 | SocketRecvHandleAndData | IPC | [`sys_socket_recv_handle_and_data`](../../kernel/src/ipc/syscall.rs#L785) |
| 640 | EventHandlerRegisterNative (public: EventHandlerRegister) | EVENT | [`sys_event_handler_register_native`](../../kernel/src/ipc/syscall.rs#L877) |
| 641 | EventHandlerUnregisterNative (public: EventHandlerUnregister) | EVENT | [`sys_event_handler_unregister_native`](../../kernel/src/ipc/syscall.rs#L998) |
| 642 | EventMask | EVENT | [`sys_event_mask`](../../kernel/src/ipc/syscall.rs#L1040) |
| 643 | EventReturn | EVENT | [`sys_event_return`](../../kernel/src/ipc/syscall.rs#L1137) |
| 644 | EventHandlerRegisterNativeWithRestorer (public: EventHandlerRegisterWithRestorer) | EVENT | [`sys_event_handler_register_native_with_restorer`](../../kernel/src/ipc/syscall.rs#L930) |
| 700 | MemoryMap | VM | [`sys_memory_map`](../../kernel/src/object/capability/memory_mapping/syscall.rs#L104) |
| 701 | MemoryUnmap | VM | [`sys_memory_unmap`](../../kernel/src/object/capability/memory_mapping/syscall.rs#L440) |
| 900 | SocketCreate | NETWORK | [`sys_socket_create`](../../kernel/src/network/syscall.rs#L547) |
| 901 | SocketBind | NETWORK | [`sys_socket_bind`](../../kernel/src/network/syscall.rs#L681) |
| 902 | SocketListen | NETWORK | [`sys_socket_listen`](../../kernel/src/network/syscall.rs#L845) |
| 903 | SocketConnect | NETWORK | [`sys_socket_connect`](../../kernel/src/network/syscall.rs#L894) |
| 904 | SocketAccept | NETWORK | [`sys_socket_accept`](../../kernel/src/network/syscall.rs#L966) |
| 905 | Socketpair | NETWORK | [`sys_socketpair`](../../kernel/src/network/syscall.rs#L1042) |
| 906 | SocketShutdown | NETWORK | [`sys_socket_shutdown`](../../kernel/src/network/syscall.rs#L1120) |
| 907 | SocketRecvFrom | NETWORK | [`sys_socket_recvfrom`](../../kernel/src/network/syscall.rs#L1241) |
| 908 | SocketSendTo | NETWORK | [`sys_socket_sendto`](../../kernel/src/network/syscall.rs#L1326) |
| 909 | SocketBindInterface | NETWORK | [`sys_socket_bind_interface`](../../kernel/src/network/syscall.rs#L790) |
| 910 | NetworkSetIpv4 | NETWORK | [`sys_network_set_ipv4`](../../kernel/src/network/syscall.rs#L201) |
| 911 | NetworkSetGateway | NETWORK | [`sys_network_set_gateway`](../../kernel/src/network/syscall.rs#L234) |
| 913 | NetworkSetNetmask | NETWORK | [`sys_network_set_netmask`](../../kernel/src/network/syscall.rs#L250) |
| 914 | NetworkListInterfaces | NETWORK | [`sys_network_list_interfaces`](../../kernel/src/network/syscall.rs#L269) |
| 915 | NetworkConfigureIpv4 | NETWORK | [`sys_network_configure_ipv4`](../../kernel/src/network/syscall.rs#L360) |
| 916 | NetworkListInterfacesV2 | NETWORK | [`sys_network_list_interfaces_v2`](../../kernel/src/network/syscall.rs#L424) |
| 917 | NetworkClearIpv4 | NETWORK | [`sys_network_clear_ipv4`](../../kernel/src/network/syscall.rs#L511) |
| 918 | SocketGetLocalAddress | NETWORK | [`sys_socket_get_local_address`](../../kernel/src/network/syscall.rs#L1201) |
| 919 | SocketGetPeerAddress | NETWORK | [`sys_socket_get_peer_address`](../../kernel/src/network/syscall.rs#L1215) |
| 997 | GetCpuDebugInfo | SNAPSHOT | [`sys_get_cpu_debug_info`](../../kernel/src/task/syscall.rs#L2309) |
| 998 | GetTaskDebugInfo | SNAPSHOT | [`sys_get_task_debug_info`](../../kernel/src/task/syscall.rs#L2170) |
| 999 | ProfilerDump | SCALAR | [`sys_profiler_dump`](../../kernel/src/syscall/mod.rs#L137) |
| 1000 | Shutdown | SCALAR | [`sys_shutdown`](../../kernel/src/task/syscall.rs#L1929) |
| 1100 | ShvVmCreate | SHV | [`sys_shv_vm_create`](../../kernel/src/hypervisor/syscall.rs#L19) |
| 1101 | ShvVcpuCreate | SHV | [`sys_shv_vcpu_create`](../../kernel/src/hypervisor/syscall.rs#L53) |
| 1102 | ShvVcpuRun | SHV | [`sys_shv_vcpu_run`](../../kernel/src/hypervisor/syscall.rs#L95) |
| 1200 | LsmLoad | LSM | [`sys_lsm_load`](../../kernel/src/lsm/syscall.rs#L38) |
| 1201 | LsmUnload | LSM | [`sys_lsm_unload`](../../kernel/src/lsm/syscall.rs#L124) |
| 1202 | LsmList | LSM | [`sys_lsm_list`](../../kernel/src/lsm/syscall.rs#L140) |
| 1300 | EnvironmentCreate | ENV | [`sys_environment_create`](../../kernel/src/executor/syscall.rs#L156) |
| 1301 | EnvironmentSetRoot | ENV | [`sys_environment_set_root`](../../kernel/src/executor/syscall.rs#L165) |
| 1302 | EnvironmentRemoveRoot | ENV | [`sys_environment_remove_root`](../../kernel/src/executor/syscall.rs#L172) |
| 1303 | EnvironmentSeal | ENV | [`sys_environment_seal`](../../kernel/src/executor/syscall.rs#L176) |
| 1304 | EnvironmentCurrent | ENV | [`sys_environment_current`](../../kernel/src/executor/syscall.rs#L180) |
| 1305 | EnvironmentGetRoot | ENV | [`sys_environment_get_root`](../../kernel/src/executor/syscall.rs#L194) |
| 1306 | EnvironmentSpawn | ENV_EXEC | [`sys_environment_spawn`](../../kernel/src/executor/syscall.rs#L455) |
| 1307 | EnvironmentExec | ENV_EXEC | [`sys_environment_exec`](../../kernel/src/executor/syscall.rs#L426) |
