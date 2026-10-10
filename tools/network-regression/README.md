# Network runtime regression checks

Run `python3 tools/test-network-regression.py` from Scarlet. It extracts code
from the current kernel source and compiles five host test suites (62 tests).
No Switch project, local kernel patch series, or dependency-cache mutation is
required. The packet serialization comparison reads the baseline source from
Git commit `6fa4a4ac2c4a1b05034057b16f614736a44344b2`; it does not apply patches.

Coverage includes NCM alignment, datagram limits, allocation counts, DMA-buffer
ownership across disconnect and slot replacement, completion accounting,
bounded queues and worker passes, packet metadata/serialization, and TCP bulk
receive semantics including wraparound, partial reads, FIN and reset.

IRQ handling, DMA allocation, scheduler wakes and device callbacks are modeled.
The tests exercise extracted production methods but do not establish physical
cache coherency, scheduling latency, throughput or long-running stability.
Generated code and receipts are written to `.cache/network-regression`; test
binaries are removed at completion.

The Switch integration of these changes completed one 8 MiB transfer per
direction at 134.87 Mbps receive and 116.44 Mbps transmit. Receive throughput
remains constrained. Those single observations are not a controlled A/B test.

`/dev/net_profile` is disabled by default. Its optional sampled timings include
preemption and overlap; they must not be summed or interpreted as CPU busy time.
