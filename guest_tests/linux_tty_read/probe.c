/* Linux AArch64: an empty nonblocking tty is EAGAIN, never EOF/EPERM. */
static long call(long n, long a, long b, long c, long d) {
  register long x0 __asm__("x0") = a, x1 __asm__("x1") = b,
                   x2 __asm__("x2") = c, x3 __asm__("x3") = d,
                   x8 __asm__("x8") = n;
  __asm__ volatile("svc #0"
                   : "+r"(x0)
                   : "r"(x1), "r"(x2), "r"(x3), "r"(x8)
                   : "memory");
  return x0;
}
static char buffer[8192] __attribute__((aligned(4096)));
static int failures;
static void check(const char *name, long actual, long expected) {
  long n = 0;
  while (name[n])
    n++;
  call(64, 1, (long)name, n, 0);
  char out[] = " actual=0000000000000000 expected=0000000000000000\n";
  for (int i = 0; i < 16; i++) {
    out[23 - i] = "0123456789abcdef"[((unsigned long)actual >> (4 * i)) & 15];
    out[49 - i] = "0123456789abcdef"[((unsigned long)expected >> (4 * i)) & 15];
  }
  call(64, 1, (long)out, sizeof(out) - 1, 0);
  if (actual != expected)
    failures++;
}
int main(void) {
  long flags = call(25, 0, 3, 0, 0);
  if (flags < 0)
    return 99;
  call(25, 0, 4, flags & ~0x800, 0);
  long fd = call(23, 0, 0, 0, 0);
  if (fd < 0)
    return 98;
  /* O_NONBLOCK is shared by duplicates. Scarlet's local fd flags were stale. */
  check("set nonblocking", call(25, 0, 4, flags | 0x800, 0), 0);
  check("single-page empty tty", call(63, fd, (long)buffer, 1, 0), -11);
  check("cross-page empty tty", call(63, fd, (long)(buffer + 16), 4096, 0),
        -11);
  check("zero-length read", call(63, fd, (long)buffer, 0, 0), 0);
  call(25, 0, 4, flags, 0);
  call(57, fd, 0, 0, 0);
  check("failures", failures, 0);
  return failures;
}
__asm__(".global _start\n_start:\nbl main\nmov x8, #94\nsvc #0\nb .\n");
