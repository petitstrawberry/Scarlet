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
