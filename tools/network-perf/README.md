# QEMU network performance loop

This fixture builds the main repository's kernel and native userspace and runs
finite TCP transfers in both directions. It tests `virtio-net` directly or the
shared xHCI / CDC-NCM / TCP path through QEMU's `usb-ncm` device. Every payload
byte is checked at its destination, and the guest returns a byte-count receipt.
It does not deploy to a board or measure a physical NIC's line rate.

## Run

Use the repository's development shell: its QEMU includes `usb-ncm`.

```sh
nix develop --command python3 tools/test-network-perf.py \
  --accel hvf --mode both --cpus 4 --bytes 67108864 --repeats 3
```

On Apple Silicon the default accelerator is HVF; on other hosts it is TCG.
An explicitly selected, unavailable accelerator fails rather than falling
back. Receipts include the accelerator and complete QEMU command. Use TCG for
scheduler race reproduction, and compare throughput only within the same
accelerator, CPU count, backend, payload size and diagnostic settings.

The listener is forwarded only on `127.0.0.1:18081`; change it with `--port`.
Both modes use QEMU user networking (SLiRP). HVF accelerates guest CPU execution;
it does not eliminate software device and network backend work. No Linux guest
reference has been measured, so these results do not establish the backend's
maximum throughput.

Options:

- `--profile`: enable sampled `/dev/net_profile` spans and dump them to serial.
- `--capture`: write a pcap; disabled by default because packet recording adds
  disk I/O to the QEMU process. Compare captured and uncaptured runs separately.
- `--build-only`: prepare the Image and initramfs without launching QEMU.
- `--no-build`: freeze and run the last build, whose hashes remain in the receipt.
- `--artifacts PATH`: run a previous frozen Image/initramfs/build-source directory
  under current measurement settings, without replacing the current sources.
- `--label NAME`: select a result directory; existing labels are never overwritten.

Outputs live in `.build/network-perf/<label>/{virtio,ncm}/`: serial logs and
`receipt.json`, plus an optional pcap. `<label>/artifacts/` retains the exact
Image, initramfs and kernel/fixture source hashes shared by both modes. Build
caches are separate and can be deleted without losing receipts or images.
Host elapsed time includes destination validation and the byte-count receipt.
Guest elapsed time, syscall counts and aggregate guest CPU busy/idle time are
recorded separately. Process CPU time is the sum of QEMU's CPU usage, not a
percentage of one vCPU.

## Fixture boot

`kernel.rs` supplies a direct ARM64 Image header and then uses Scarlet's normal
Linux boot entry. Direct boot has no firmware PCI allocator. This fixture alone
owns PCI slot `00:01.0` and assigns its xHCI BAR to `0x10000000` before kernel
mapping and enumeration. The production PCI or board initialization code is
not changed. The local kernel dependency disables default features and enables
`linux-boot`, `network`, `user-fpu` and `user-vector`. The initramfs contains the
normal native init, the network test server, and Scarlet's dynamic loader.

## Findings and changes

A lock-free trace under four-vCPU TCG caught a wake before context save: CPU 0
queued a still-owned blocked task on CPU 1; CPU 1 consumed its reschedule IPI and
rejected the task because CPU 2 still owned its context. CPU 2 then requeued it
without a new notification, and did not claim it until the idle watchdog.
The trace records context release at 619268 microseconds, local requeue at
619292, and claim at 864072: a 244780-microsecond avoidable gap. Wakes now stay
on the context owner until switch-out releases ownership, subject to affinity
and deadline eligibility. Normal target selection resumes after release.

The TCP receive-window notification now accumulates drains relative to the
window edge offered to the peer, rather than forgetting each small drain.
Socket lookup shares immutable registration snapshots instead of allocating a
new vector for every incoming segment. ACK/SYN/FIN serialization and small IPv4
encapsulation use stack storage; interface metadata is borrowed where possible.
The final Ethernet frame and queued packets still own their storage.

Application-write serialization previously masked interrupts across the entire
payload, including all MSS-sized segments of a 64 KiB write. It now uses a
preemption guard without masking interrupts: ACK, retransmission and IRQ paths
do not acquire this outer lock. The individual shared socket/device structures
keep their IRQ guards. This removes the long IRQ blackout while preserving
write ordering and rollback.

Virtio previously allocated PMM pages and waited for TX completion with
interrupts masked on every frame, including ACKs. It now keeps one DMA buffer
per hardware descriptor, retains a bounded FIFO when descriptors are occupied,
and releases descriptor ownership only on a used-ring completion. IRQ handling
reclaims completions and resumes pending TX. Out-of-order completions, queue
saturation, malformed lengths and duplicate completions are covered by tests.
Device teardown resets before releasing DMA, retaining storage if reset fails.

Virtio also read link configuration through MMIO per send and overwrote the
device-owned `used.flags` during RX recycling. Carrier is now refreshed on
configuration interrupts, and doorbells respect notification suppression after
a publication barrier. These ownership and caching rules follow the
[VirtIO 1.2 specification](https://docs.oasis-open.org/virtio/virtio/v1.2/virtio-v1.2.html),
sections 2.7.10, 2.7.13, 3.2 and 3.3. EVENT_IDX remains unnegotiated.

## Validation and measurements

The ARM64 kernel QEMU suite passed all **1433 tests**, including wake ownership,
affinity, receive-window wrap/reopening, stable socket snapshots, delayed TX
completion, bounded queue ownership and interrupt-driven carrier changes. The
existing host NCM/DMA/packet/worker/TCP-drain harness passed **62 tests**.
The RISC-V suite, including real MMIO and hub-connected virtio devices, passed
all **1449 tests** after updating the owned-packet TX API call sites.
After reporting success, RISC-V QEMU printed `virtio: zero sized buffers are
not allowed`; the suite exited 0. Its cause is not established here. The final
HVF transfer logs do not contain that warning. Raw validation logs are retained
at the paths listed in the results JSON.

Machine-readable results are in `results/2026-10-10.json`. The initial TCG
investigation reproduced roughly 250ms scheduler stalls; after fixing context
ownership, four-CPU virtio RX sustained approximately 110–113 Mbps and NCM RX
257–273 Mbps. Those diagnostic TCG runs are separate from HVF throughput.

The HVF before/after comparison uses four vCPUs, SLiRP, no pcap or sampled
profiling, and three transfers per direction. The "before virtio/control"
image already contains the scheduler, receive-window and socket-snapshot fixes.
The final image additionally contains async pooled virtio TX, notification and
carrier fixes, small-control-packet allocation reductions, and application-write
serialization without the outer IRQ mask. See the JSON for
all samples, byte sizes and image hashes; improvements in the virtio path must
not be attributed to Switch's physical USB/NIC path.

64 MiB comparison (Mbps, median of three):

| Backend | Direction | Before virtio/control | Final |
|---|---|---:|---:|
| virtio | guest RX | 296.36 | 518.26 |
| virtio | guest TX | 277.57 | 475.90 |
| USB-NCM | guest RX | 715.88 | 781.64 |
| USB-NCM | guest TX | 358.81 | 376.88 |

These rows compare `hvf-baseline-64m` with `hvf-tcp-irq-quiet`. Virtio process
CPU time per 64 MiB drops from median 3.41 to 1.86 seconds for RX and from
4.59 to 1.65 seconds for TX. Payload validation remains enabled in every run.

Virtio improves consistently across the 16 MiB and 64 MiB comparisons. NCM
improved in one 16 MiB control-allocation run, but decreased in the following
64 MiB comparison before the outer TCP IRQ mask was removed. Those samples
remain published. A frozen-image recheck after the final quiet run gives
715.33/349.51 Mbps RX/TX with the outer mask, then 771.83/376.36 Mbps without
it. This supports a modest NCM improvement; it does not establish a contribution
from the allocation changes alone. The first `hvf-tcp-irq-unmasked` run overlapped
kernel compilation/testing and is explicitly excluded from the clean comparison.
Do not run throughput measurements concurrently with the test suites.

RX continues
to wake/read very frequently, and NCM TX is still slower than direct virtio.
Neither backend has reached 1 Gbps in this fixture. Physical Switch throughput,
latency and concurrent video/SSH behavior remain to be tested separately.

`hvf-ncm-remaining-profile` preserves a 16 MiB sampled run. Its cumulative
spans identify TCP receive/stack dispatch and queue waiting as more expensive
than DMA copying; they include preemption and lock waits, overlap, and must not
be added together or treated as CPU time. Further work should separate ACK
processing, socket locks/wakes and send-side packet construction.
