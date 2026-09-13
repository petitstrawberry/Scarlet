# Linux ABI userspace checks

Build these programs with an ordinary Linux compiler for the guest architecture.
Run them inside Scarlet's matching Linux Environment, for example:

```sh
cc -O3 clock-sleep.c -o clock-sleep
abi-run linux-aarch64 /bin/clock-sleep
```

Install the executable in the Linux view's `/bin`, backed by
`/systems/linux-aarch64/bin` in the default Environment. The clock check exercises
real relative and absolute deadlines, Linux error returns, pointer validation,
and the shared nanosleep implementation. It does not verify userspace signal
handler delivery or realtime clock adjustments.

The thread-signal check uses ordinary musl pthreads and credential functions:

```sh
cc -O3 -Wall -Wextra -pthread thread-signals.c -o thread-signals
# First run on Linux, then install it in Scarlet's Linux view.
./thread-signals
abi-run linux-aarch64 /bin/thread-signals --expect-unavailable
```

Scarlet currently cannot execute custom userspace handlers through `tkill` or
`tgkill`. These sends must return `ENOSYS`, including masked sends to another
thread in the same group, rather than claim successful delivery. Otherwise musl's
synchronous credential broadcast waits forever for a handler that never runs.
The check verifies explicit errors, signal-zero probes, a default ignored signal,
prompt libc broadcast failure, unchanged credentials, and successful thread
joins. It does not establish general signal-handler or credential support.

The mapping check repeatedly touches and partially unmaps an 8 MiB anonymous
region, preserving guard pages on both sides and replacing the removed range
at the same address. It checks zeroed replacement pages and reports the actual
unmap time, without imposing a machine-dependent performance threshold:

```sh
cc -O3 -Wall -Wextra mapping-churn.c -o mapping-churn
./mapping-churn
abi-run linux-aarch64 /bin/mapping-churn
```

The `mremap` check uses the same unaligned lengths as a game hunk allocator,
touches retained and discarded pages, and reuses the freed tail without
`MAP_FIXED`. It verifies retained data, new zeroed backing, same-page resizing,
and invalid arguments:

```sh
cc -O3 -Wall -Wextra mremap-shrink.c -o mremap-shrink
./mremap-shrink
abi-run linux-aarch64 /bin/mremap-shrink
```

Scarlet supports in-place shrinking with flags zero or `MREMAP_MAYMOVE`.
Growth, relocation, `MREMAP_FIXED`, `MREMAP_DONTUNMAP`, and zero-length shared
mapping duplication are not implemented. Unsupported resize operations return
an error and leave the source mapping intact.
