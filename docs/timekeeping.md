# Wall-clock time and network synchronization

Scarlet keeps two independent clocks:

- `MonotonicTime` measures elapsed boot time. Scheduling, timeouts and media
  synchronization must use this clock. UTC adjustments do not change it.
- `SystemTime` reports Unix nanoseconds. The first RTC sample seeds it, and
  userspace can subsequently step it forward or backward. Until either source
  initializes it, the Native ABI returns `u64::MAX`.

## Updating UTC

Native syscall 52, `SetSystemTime`, accepts a pointer and an exact 24-byte size.
The `RawSystemTimeUpdateV1` record contains `version = 1`, `reserved = 0`, UTC
nanoseconds and the monotonic instant at which that UTC value was valid. The
kernel accounts for time elapsed since that instant, and publishes the pair
under an IRQ-safe lock. Future references, overflow, unsupported versions and
nonzero reserved fields fail without changing the clock. No RTC is written.

Only init's thread group may invoke this syscall. The high-level wrapper is
`scarlet_os::time::set_system_time_at`. Trusted local services use stemd's
`SET_SYSTEM_TIME` command (0x07), followed by UTC and monotonic nanoseconds as
two little-endian u64 values. Like stemd's shutdown interface, this currently
relies on the local service trust boundary; it does not authenticate peers.
The kernel API is independent of NTP, any network driver, and any RTC model.

## ntpd

The base bundle starts `/bin/ntpd` after resolverd, without holding up boot for
network connectivity. It implements the unicast SNTP subset of NTPv4 over
IPv4 UDP port 123, accepting NTPv3/v4 server replies. Its default server is
[`time.cloudflare.com`](https://developers.cloudflare.com/time-services/ntp/usage/).

```sh
/bin/ntpd --query                 # show one sample without applying it
/bin/ntpd --once                  # synchronize once, exit nonzero on failure
/bin/ntpd --server ntp.example.org --interval 1024
/bin/logctl -u ntpd -n 20
/bin/date
```

Override the service's `exec` in `/etc/stemd.d/services/04-ntpd.toml` to select
a different server or interval. Avoid running a second daemon alongside it.
The successful default polling interval is 1024 seconds. Offline failures
retry at 16, 32, 64 seconds and so on up to the configured interval. Server
rate-limit requests increase the minimum interval; DENY/RSTR stops further
requests until the daemon is restarted. Requests use an ephemeral source port
and are compatible with host NAT.

Replies must match the queried source address/port and request cookie, carry
valid mode/version, leap and stratum fields, and contain nonzero, consistent
receive/transmit timestamps. Exchange time is measured with the monotonic
clock. Server processing time is removed from the round trip, and half of the
remaining network delay is added to the transmit time. This works even when
local UTC is unset or years wrong. Timestamp unfolding handles the 2036 era
rollover in the RFC 4330 window (Unix epoch through 2104).

This is a leaf SNTP client following [RFC 4330](https://www.rfc-editor.org/rfc/rfc4330.html),
not a full NTP frequency-discipline or multi-source selection implementation.
It steps UTC rather than slewing it. Plain NTP is unauthenticated; the cookie
correlates replies but does not provide NTS security. An offline boot retains
the RTC-derived date until a successful synchronization.

On Switch, the MAX77620 driver reads the raw calendar and does not interpret
Horizon's separately stored time offset. Network correction therefore applies
to Scarlet's software clock without changing the calendar seen by other OSes.

## Switch validation, 2026-09-21

The protocol, clock state and Native ABI checks passed (5 + 2 + 8 tests).
The AArch64 kernel, userspace, initramfs and rootfs images built successfully.
On hardware, the new service initially found resolverd unavailable, retried
after 16 seconds and synchronized with `162.159.200.123:123` (stratum 3,
network delay 7466 microseconds). The date changed from 2000-01-08 to the
current 2026-09-21 Japan time without user clock input. A separate process
verified that direct clock writes are denied outside init, and that wall time
and monotonic time each advanced by approximately one second during sleep.

The SD package passed readback and the ext2 root has the matching `stemd`,
`ntpd` and startup configuration. HTTP continued to return 200 after the clock
step. HTTPS still returned `FailedToGetRandomBytes`, identifying a separate
missing cryptographic entropy source on this board; NTP does not relax TLS or
SSH randomness requirements. Test artifacts and UART output are in the Switch
project's `.cache/ntp-bringup-20260921/` directory.
