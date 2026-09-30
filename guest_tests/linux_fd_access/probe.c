/* Regression: read-only I/O and dup(CLOEXEC) followed by exec, Linux AArch64.
 */
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
static long report;
static int failed;
static void check(const char *name, long got, long want) {
  long n = 0;
  while (name[n])
    n++;
  call(64, report, (long)name, n, 0);
  char s[] = " actual=0000000000000000 expected=0000000000000000\n";
  for (int i = 0; i < 16; i++) {
    s[23 - i] = "0123456789abcdef"[((unsigned long)got >> (i * 4)) & 15];
    s[49 - i] = "0123456789abcdef"[((unsigned long)want >> (i * 4)) & 15];
  }
  call(64, report, (long)s, sizeof(s) - 1, 0);
  if (got != want)
    failed++;
}
struct iovec {
  void *base;
  unsigned long size;
};
int main(long argc, char **argv) {
  report = call(56, -100, (long)"/tmp/fd-access-probe.log",
                argc > 1 ? 02001 : 01101, 0600);
  if (report < 0)
    return 99;
  if (argc > 1) {
    failed = argv[2][0] == '1';
    long dupfd = 0;
    for (char *p = argv[1]; *p; p++)
      dupfd = dupfd * 10 + *p - '0';
    check("dup survives exec", call(25, dupfd, 1, 0, 0), 0);
    check("dup3 survives exec", call(25, 64, 1, 0, 0), 0);
    check("dup3 CLOEXEC closes", call(25, 65, 1, 0, 0), -9);
    check("original CLOEXEC closes", call(25, 66, 1, 0, 0), -9);
    char byte;
    check("dup retains read access", call(63, 64, (long)&byte, 1, 0), 1);
    check("dup retains write restriction", call(64, 64, (long)&byte, 1, 0), -9);
    check("total failures", failed, 0);
    return failed;
  }
  long fd = call(56, -100, (long)"/tmp/fd-access-probe.data", 01102, 0600);
  char buffer[8] = "seed";
  struct iovec vec = {buffer, 4};
  check("seed write", call(64, fd, (long)buffer, 4, 0), 4);
  call(57, fd, 0, 0, 0);
  fd = call(56, -100, (long)"/tmp/fd-access-probe.data", 0, 0);
  check("write RO", call(64, fd, (long)"oops", 4, 0), -9);
  check("pwrite RO", call(68, fd, (long)"oops", 4, 0), -9);
  check("writev RO", call(66, fd, (long)&vec, 1, 0), -9);
  check("pwritev RO", call(70, fd, (long)&vec, 1, 0), -9);
  check("zero write RO", call(64, fd, (long)buffer, 0, 0), -9);
  check("RO read", call(67, fd, (long)buffer, 4, 0), 4);
  check("contents preserved",
        buffer[0] == 's' && buffer[1] == 'e' && buffer[2] == 'e' &&
            buffer[3] == 'd',
        1);
  call(57, fd, 0, 0, 0);
  fd = call(56, -100, (long)"/tmp/fd-access-probe.data", 1, 0);
  check("read WO", call(63, fd, (long)buffer, 4, 0), -9);
  check("pread WO", call(67, fd, (long)buffer, 4, 0), -9);
  check("readv WO", call(65, fd, (long)&vec, 1, 0), -9);
  check("preadv WO", call(69, fd, (long)&vec, 1, 0), -9);
  call(57, fd, 0, 0, 0);
  fd = call(56, -100, (long)"/tmp/fd-access-probe.data", 0, 0);
  check("save CLOEXEC source", call(24, fd, 66, 02000000, 0), 66);
  call(57, fd, 0, 0, 0);
  long dupfd = call(23, 66, 0, 0, 0);
  check("dup clears flag", call(25, dupfd, 1, 0, 0), 0);
  check("dup3 clears flag", call(24, 66, 64, 0, 0), 64);
  check("dup3 sets flag", call(24, 66, 65, 02000000, 0), 65);
  check("dup3 same fd", call(24, 66, 66, 0, 0), -22);
  check("dup3 invalid flags", call(24, 66, 64, 1, 0), -22);
  check("stage1 failures", failed, 0);
  char num[24];
  int i = 23;
  num[i] = 0;
  do {
    num[--i] = '0' + dupfd % 10;
    dupfd /= 10;
  } while (dupfd);
  char *args[] = {"/tmp/fd-access-probe", num + i, failed ? "1" : "0", 0};
  char *env[] = {0};
  call(57, report, 0, 0, 0);
  return call(221, (long)args[0], (long)args, (long)env, 0);
}
__asm__(".global _start\n_start:\nldr x0, [sp]\nadd x1, sp, #8\nbl main\nmov "
        "x8, #94\nsvc #0\nb .\n");
