/* Linux AArch64: independently opened terminals must not share O_NONBLOCK. */
typedef unsigned long usize;
typedef long isize;
static isize sc(usize n, usize a, usize b, usize c, usize d, usize e, usize f) {
    register usize x8 __asm__("x8") = n, x0 __asm__("x0") = a,
                   x1 __asm__("x1") = b, x2 __asm__("x2") = c,
                   x3 __asm__("x3") = d, x4 __asm__("x4") = e,
                   x5 __asm__("x5") = f;
    __asm__ volatile("svc 0" : "+r"(x0) : "r"(x8), "r"(x1), "r"(x2),
                     "r"(x3), "r"(x4), "r"(x5) : "memory", "cc");
    return (isize)x0;
}
static unsigned failures;
static char buffer[8192] __attribute__((aligned(4096)));
struct termios { unsigned iflag, oflag, cflag, lflag; unsigned char line, cc[19]; };
struct iovec { void *base; usize len; };
static void print(const char *s) {
    usize n = 0; while (s[n]) ++n;
    sc(64, 1, (usize)s, n, 0, 0, 0);
}
static void expect(const char *name, isize got, isize wanted) {
    print(got == wanted ? "PASS " : "FAIL "); print(name); print("\n");
    if (got != wanted) ++failures;
}
static isize open_tty(const char *path, int nonblocking) {
    return sc(56, -100, (usize)path, 2 | 0x100 | (nonblocking ? 0x800 : 0), 0, 0, 0);
}
static isize mode(int fd) { return sc(25, fd, 3, 0, 0, 0, 0) & 0x800; }
static isize set_mode(int fd, int nonblocking) {
    isize flags = sc(25, fd, 3, 0, 0, 0, 0);
    return sc(25, fd, 4, (flags & ~0x800) | (nonblocking ? 0x800 : 0), 0, 0, 0);
}
static void blocking_read(int master, int slave, usize offset, usize length, int vector) {
    isize child = sc(220, 17, 0, 0, 0, 0, 0);
    if (child == 0) {
        struct iovec iov = {buffer + offset, length};
        isize n = vector ? sc(65, slave, (usize)&iov, 1, 0, 0, 0)
                         : sc(63, slave, (usize)(buffer + offset), length, 0, 0, 0);
        sc(94, n == 6 && buffer[offset] == 'h' && buffer[offset+5] == '\n' ? 0 : 1, 0, 0, 0, 0, 0);
        for (;;) {}
    }
    if (child < 0) { expect("fork blocking reader", child, 0); return; }
    isize delay[2] = {0, 200000000};
    sc(101, (usize)delay, 0, 0, 0, 0, 0);
    unsigned status = 0;
    isize waited = sc(260, child, (usize)&status, 1, 0, 0, 0);
    expect("blocking reader waits for delayed input", waited, 0);
    if (waited == 0) {
        expect("feed complete canonical line", sc(64, master, (usize)"hello\n", 6, 0, 0, 0), 6);
        expect("reap blocking reader", sc(260, child, (usize)&status, 0, 0, 0, 0), child);
    }
    expect("blocking reader receives the line", status, 0);
}
int main(void) {
    int master = open_tty("/dev/ptmx", 0);
    if (master < 0) { expect("open ptmx", master, 0); return 1; }
    unsigned number = 0;
    int unlocked = 0;
    expect("get PTY number", sc(29, master, 0x80045430, (usize)&number, 0, 0, 0), 0);
    expect("unlock PTY", sc(29, master, 0x40045431, (usize)&unlocked, 0, 0, 0), 0);
    char path[32] = "/dev/pts/";
    unsigned pos = 9;
    char digits[12]; unsigned count = 0;
    do { digits[count++] = '0' + number % 10; number /= 10; } while (number);
    while (count) path[pos++] = digits[--count]; path[pos] = 0;
    int blocking = open_tty(path, 0), other = open_tty(path, 1);
    if (blocking < 0 || other < 0) { expect("open slaves", -1, 0); return 1; }
    struct termios t;
    expect("read shared terminal settings", sc(29, blocking, 0x5401, (usize)&t, 0, 0, 0), 0);
    t.lflag = (t.lflag | 2) & ~8u; t.cc[6] = 1; t.cc[5] = 0;
    expect("set canonical mode", sc(29, blocking, 0x5402, (usize)&t, 0, 0, 0), 0);
    expect("original open remains blocking", mode(blocking), 0);
    expect("new open is nonblocking", mode(other), 0x800);
    int alias = sc(23, other, 0, 0, 0, 0, 0);
    expect("clear nonblocking through dup", set_mode(alias, 0), 0);
    expect("original observes dup clearing nonblocking", mode(other), 0);
    expect("set nonblocking through original", set_mode(other, 1), 0);
    expect("dup observes original setting nonblocking", mode(alias), 0x800);
    expect("single-page empty terminal is EAGAIN", sc(63, alias, (usize)buffer, 1, 0, 0, 0), -11);
    expect("cross-page empty terminal is EAGAIN", sc(63, alias, (usize)(buffer+4032), 128, 0, 0, 0), -11);
    struct iovec iov = {buffer, 8};
    expect("readv on nonblocking alias is EAGAIN", sc(65, alias, (usize)&iov, 1, 0, 0, 0), -11);
    expect("zero-length terminal read", sc(63, alias, (usize)buffer, 0, 0, 0, 0), 0);
    isize child = sc(220, 17, 0, 0, 0, 0, 0);
    if (child == 0) {
        sc(94, set_mode(alias, 0) == 0 ? 0 : 1, 0, 0, 0, 0, 0);
        for (;;) {}
    }
    unsigned status = 0;
    expect("reap flag-changing child", sc(260, child, (usize)&status, 0, 0, 0, 0), child);
    expect("child successfully changes shared mode", status, 0);
    expect("parent observes fork child clearing nonblocking", mode(other), 0);
    set_mode(other, 1);
    expect("separate open remains blocking after fork change", mode(blocking), 0);
    blocking_read(master, blocking, 0, 8, 0);
    blocking_read(master, blocking, 4032, 128, 0);
    blocking_read(master, blocking, 4032, 128, 1);
    unsigned char eof = t.cc[4];
    expect("feed canonical EOF", sc(64, master, (usize)&eof, 1, 0, 0, 0), 1);
    expect("real EOF stays a zero-byte read", sc(63, blocking, (usize)buffer, 1, 0, 0, 0), 0);
    sc(57, alias, 0, 0, 0, 0, 0); sc(57, other, 0, 0, 0, 0, 0);
    sc(57, blocking, 0, 0, 0, 0, 0); sc(57, master, 0, 0, 0, 0, 0);
    print(failures ? "Terminal regression FAILED\n" : "Terminal regression PASSED\n");
    return failures;
}
__asm__(".global _start\n_start:\nbl main\nmov x8,#94\nsvc #0\nb .\n");
