/* Freestanding AArch64 Linux regression checks for APT/glibc prerequisites. */
typedef unsigned long usize;
typedef long isize;
typedef unsigned int u32;

static isize sc(usize n, usize a, usize b, usize c, usize d, usize e, usize f)
{
    register usize x8 __asm__("x8") = n;
    register usize x0 __asm__("x0") = a;
    register usize x1 __asm__("x1") = b;
    register usize x2 __asm__("x2") = c;
    register usize x3 __asm__("x3") = d;
    register usize x4 __asm__("x4") = e;
    register usize x5 __asm__("x5") = f;
    __asm__ volatile("svc 0" : "+r"(x0) : "r"(x8), "r"(x1), "r"(x2),
                     "r"(x3), "r"(x4), "r"(x5) : "memory", "cc");
    return (isize)x0;
}

struct iovec { void *base; usize len; };
struct msghdr {
    void *name;
    u32 namelen, pad;
    struct iovec *iov;
    usize iovlen;
    void *control;
    usize controllen;
    u32 flags, pad2;
};
struct mmsghdr { struct msghdr hdr; u32 len, pad; };

static unsigned failures;
static char pages[8192] __attribute__((aligned(4096)));

static void print(const char *s)
{
    usize n = 0;
    while (s[n]) ++n;
    sc(64, 1, (usize)s, n, 0, 0, 0);
}

static void expect(const char *name, isize got, isize wanted)
{
    print(got == wanted ? "PASS " : "FAIL ");
    print(name);
    print("\n");
    if (got != wanted) ++failures;
}

static void records(unsigned type)
{
    int pair[2] = {-1, -1};
    expect("socketpair", sc(199, 1, type | 0x800, 0, (usize)pair, 0, 0), 0);
    if (pair[0] < 0) return;
    char one[] = "first";
    char two[] = "second";
    struct iovec vectors[2] = {{one, 5}, {two, 6}};
    struct mmsghdr messages[2] = {
        {.hdr = {.iov = &vectors[0], .iovlen = 1}, .len = 0xfeed},
        {.hdr = {.iov = &vectors[1], .iovlen = 1}, .len = 0xbeef}
    };
    expect("sendmmsg returns messages, not bytes",
           sc(269, pair[0], (usize)messages, 2, 0x40, 0, 0), 2);
    expect("first msg_len", messages[0].len, 5);
    expect("second msg_len", messages[1].len, 6);
    char buffer[32];
    expect("first record length", sc(207, pair[1], (usize)buffer, 32, 0x40, 0, 0), 5);
    expect("first record payload", buffer[0], 'f');
    expect("second record length", sc(207, pair[1], (usize)buffer, 32, 0x40, 0, 0), 6);
    expect("second record payload", buffer[0], 's');
    expect("no extra record", sc(207, pair[1], (usize)buffer, 32, 0x40, 0, 0), -11);

    messages[0].len = 0xfeed;
    messages[1].len = 0xbeef;
    messages[1].hdr.iov = (void *)1;
    expect("failed second message returns successful prefix",
           sc(269, pair[0], (usize)messages, 2, 0x40, 0, 0), 1);
    expect("successful prefix length saved", messages[0].len, 5);
    expect("failed message length untouched", messages[1].len, 0xbeef);
    expect("successful prefix delivered", sc(207, pair[1], (usize)buffer, 32, 0x40, 0, 0), 5);
    expect("failed first message returns EFAULT",
           sc(269, pair[0], (usize)&messages[1], 1, 0x40, 0, 0), -14);
    expect("empty batch", sc(269, pair[0], 0, 0, 0, 0, 0), 0);
    char record[256];
    expect("cross-page write stays one record",
           sc(64, pair[0], (usize)&pages[4032], 128, 0, 0, 0), 128);
    expect("cross-page record length", sc(207, pair[1], (usize)record, 256, 0x40, 0, 0), 128);
    expect("cross-page write leaves no second record", sc(207, pair[1], (usize)record, 256, 0x40, 0, 0), -11);
    sc(57, pair[0], 0, 0, 0, 0, 0);
    sc(57, pair[1], 0, 0, 0, 0, 0);
}

static void pipe_readiness(void)
{
    int pair[2] = {-1, -1};
    struct pollfd { int fd; short events, revents; } pollfd;
    isize timeout[2] = {0, 0};
    expect("nonblocking pipe2", sc(59, (usize)pair, 0x800, 0, 0, 0, 0), 0);
    if (pair[0] < 0) return;
    expect("read endpoint access mode", sc(25, pair[0], 3, 0, 0, 0, 0) & 3, 0);
    expect("write endpoint access mode", sc(25, pair[1], 3, 0, 0, 0, 0) & 3, 1);
    expect("lseek pipe reports ESPIPE for dpkg's discard fallback", sc(62, pair[0], 0, 1, 0, 0, 0), -29);
    expect("fill pipe leaving 96 bytes", sc(64, pair[1], (usize)pages, 4000, 0, 0, 0), 4000);
    pollfd = (struct pollfd){pair[1], 4, 0};
    expect("poll does not promise atomic write into insufficient space",
           sc(73, (usize)&pollfd, 1, (usize)timeout, 0, 0, 0), 0);
    expect("cross-page atomic write returns EAGAIN",
           sc(64, pair[1], (usize)&pages[4032], 128, 0, 0, 0), -11);
    expect("no partial atomic write was delivered",
           sc(63, pair[0], (usize)pages, 8192, 0, 0, 0), 4000);
    expect("pipe now empty", sc(63, pair[0], (usize)pages, 8192, 0, 0, 0), -11);
    pollfd.revents = 0;
    expect("poll writable after draining", sc(73, (usize)&pollfd, 1, (usize)timeout, 0, 0, 0), 1);
    expect("POLLOUT bit", pollfd.revents, 4);
    expect("large nonblocking write returns successful prefix",
           sc(64, pair[1], (usize)pages, 8192, 0, 0, 0), 4096);
    expect("large write delivered exactly its prefix",
           sc(63, pair[0], (usize)pages, 8192, 0, 0, 0), 4096);
    sc(57, pair[0], 0, 0, 0, 0, 0);
    sc(57, pair[1], 0, 0, 0, 0, 0);
}

struct flock { short kind, whence; u32 pad; isize start, len; int pid; u32 pad2; };

static void exec_locks(int fd, unsigned close_alias)
{
    int pipe[2] = {-1, -1};
    expect("exec lock synchronization pipe", sc(59, (usize)pipe, 0x80000, 0, 0, 0, 0), 0);
    isize child = sc(220, 17, 0, 0, 0, 0, 0);
    if (child == 0) {
        sc(57, pipe[0], 0, 0, 0, 0, 0);
        struct flock lock = {.kind = 1};
        expect("child acquires before exec", sc(25, fd, 6, (usize)&lock, 0, 0, 0), 0);
        if (close_alias)
            expect("CLOEXEC alias of locked inode", sc(24, fd, 200, 0x80000, 0, 0, 0), 200);
        char ready = 'r';
        sc(64, pipe[1], (usize)&ready, 1, 0, 0, 0);
        char *argv[] = {"sleep", "3", 0};
        char *envp[] = {"LC_ALL=C", 0};
        isize result = sc(221, (usize)"/usr/bin/sleep", (usize)argv, (usize)envp, 0, 0, 0);
        expect("exec sleep must replace the child", result, 0);
        sc(94, 1, 0, 0, 0, 0, 0);
    }
    expect("fork for exec lock lifecycle", child > 0, 1);
    sc(57, pipe[1], 0, 0, 0, 0, 0);
    if (child <= 0) { sc(57, pipe[0], 0, 0, 0, 0, 0); return; }
    char ready;
    expect("child published locked inode", sc(63, pipe[0], (usize)&ready, 1, 0, 0, 0), 1);
    isize delay[2] = {0, 500000000};
    sc(101, (usize)delay, 0, 0, 0, 0, 0);
    int status = -1;
    isize exited = sc(260, child, (usize)&status, 1, 0, 0, 0);
    expect("exec child remains alive during lock check", exited, 0);
    struct flock lock = {.kind = 1};
    expect("GETLK after child exec", sc(25, fd, 5, (usize)&lock, 0, 0, 0), 0);
    expect(close_alias ? "CLOEXEC alias close releases all inode locks" : "exec preserves lock on retained descriptor",
           lock.kind, close_alias ? 2 : 1);
    if (!close_alias) expect("exec preserves lock owner PID", lock.pid, child);
    if (exited == 0) {
        expect("wait for exec child", sc(260, child, (usize)&status, 0, 0, 0, 0), child);
        expect("exec child completed successfully", status, 0);
    }
    sc(57, pipe[0], 0, 0, 0, 0, 0);
}

static void file_operations(void)
{
    const char *directory = "/var/tmp/scarlet-apt-probe";
    expect("mkdir on ext2", sc(34, -100, (usize)directory, 0700, 0, 0, 0), 0);
    isize d = sc(56, -100, (usize)directory, 0x10000, 0, 0, 0);
    expect("open directory fd", d >= 0, 1);
    if (d < 0) return;
    isize fd = sc(56, d, (usize)"original", 0xc2, 0600, 0, 0);
    expect("create original", fd >= 0, 1);
    if (fd < 0) return;
    expect("write original", sc(64, fd, (usize)"old", 3, 0, 0, 0), 3);
    expect("linkat relative to directory fd", sc(37, d, (usize)"original", d, (usize)"linked", 0, 0), 0);
    expect("linkat existing target", sc(37, d, (usize)"original", d, (usize)"linked", 0, 0), -17);
    expect("linkat invalid flags", sc(37, d, (usize)"original", d, (usize)"other", 8, 0), -22);
    expect("linkat directory rejected", sc(37, d, (usize)".", d, (usize)"other", 0, 0), -1);
    expect("symlinkat relative target", sc(36, (usize)"original", d, (usize)"symlink", 0, 0, 0), 0);
    expect("linkat symlink without following", sc(37, d, (usize)"symlink", d, (usize)"symlink-copy", 0, 0), 0);
    char target[32];
    expect("hardlinked symlink retains target", sc(78, d, (usize)"symlink-copy", (usize)target, 32, 0, 0), 8);
    expect("hardlinked symlink target bytes", target[0], 'o');
    expect("linkat follows only with AT_SYMLINK_FOLLOW", sc(37, d, (usize)"symlink", d, (usize)"followed", 0x400, 0), 0);
    unsigned char statbuf[256] __attribute__((aligned(8)));
    expect("statx linked inode", sc(291, d, (usize)"followed", 0, 0x7ff, (usize)statbuf, 0), 0);
    expect("three links to original inode", *(u32 *)&statbuf[16], 3);
    expect("fchownat root ownership", sc(54, d, (usize)"original", 0, 0, 0, 0), 0);
    expect("fchownat leaves ownership unchanged", sc(54, fd, (usize)"", -1, -1, 0x1000, 0), 0);
    expect("fchownat rejects unsupported non-root identity", sc(54, d, (usize)"original", 1000, 0, 0, 0), -95);
    expect("fchownat missing path", sc(54, d, (usize)"missing", 0, 0, 0, 0), -2);
    expect("fchownat no-follow dangling symlink", sc(54, d, (usize)"symlink", 0, 0, 0x100, 0), 0);
    for (unsigned i = 0; i < sizeof("original"); ++i) pages[4092+i] = "original"[i];
    expect("fchmodat reads cross-page pathname", sc(53, d, (usize)&pages[4092], 0640, 0, 0, 0), 0);
    expect("statx changed mode", sc(291, d, (usize)"original", 0, 0x7ff, (usize)statbuf, 0), 0);
    expect("chmod mode visible", *(unsigned short *)&statbuf[28] & 0777, 0640);

    struct flock lock = {.kind = 1};
    expect("POSIX whole-file write lock", sc(25, fd, 6, (usize)&lock, 0, 0, 0), 0);
    lock.kind = 1;
    expect("GETLK for own lock", sc(25, fd, 5, (usize)&lock, 0, 0, 0), 0);
    expect("own process does not contend", lock.kind, 2);
    int parent = sc(172, 0, 0, 0, 0, 0, 0);
    isize child = sc(220, 17, 0, 0, 0, 0, 0);
    if (child == 0) {
        lock = (struct flock){.kind = 1};
        expect("fork child does not inherit parent's lock", sc(25, fd, 6, (usize)&lock, 0, 0, 0), -11);
        expect("contended sleeping lock explicitly unsupported", sc(25, fd, 7, (usize)&lock, 0, 0, 0), -95);
        expect("GETLK reports conflict", sc(25, fd, 5, (usize)&lock, 0, 0, 0), 0);
        expect("GETLK conflicting type", lock.kind, 1);
        expect("GETLK conflicting process", lock.pid, parent);
        sc(57, fd, 0, 0, 0, 0, 0);
        sc(94, failures ? 1 : 0, 0, 0, 0, 0, 0);
    }
    expect("fork for lock contention", child > 0, 1);
    int status = -1;
    if (child > 0) {
        expect("wait for contention child", sc(260, child, (usize)&status, 0, 0, 0, 0), child);
        expect("contention child checks passed", status, 0);
    }
    isize alias = sc(56, d, (usize)"linked", 2, 0, 0, 0);
    expect("open independent alias", alias >= 0, 1);
    expect("close another fd releases process's locks", sc(57, alias, 0, 0, 0, 0, 0), 0);
    child = sc(220, 17, 0, 0, 0, 0, 0);
    if (child == 0) {
        lock = (struct flock){.kind = 1};
        expect("child acquires after parent's alias close", sc(25, fd, 6, (usize)&lock, 0, 0, 0), 0);
        sc(94, failures ? 1 : 0, 0, 0, 0, 0, 0);
    }
    if (child > 0) {
        expect("wait for lock owner exit", sc(260, child, (usize)&status, 0, 0, 0, 0), child);
        expect("exit child checks passed", status, 0);
    }
    lock = (struct flock){.kind = 1};
    expect("exit releases locks before zombie reaping", sc(25, fd, 6, (usize)&lock, 0, 0, 0), 0);
    lock.len = 1;
    expect("byte-range lock explicitly unsupported", sc(25, fd, 6, (usize)&lock, 0, 0, 0), -95);
    lock = (struct flock){.kind = 2};
    expect("unlock whole file", sc(25, fd, 6, (usize)&lock, 0, 0, 0), 0);
    exec_locks(fd, 0);
    exec_locks(fd, 1);
    expect("unlink original while open", sc(35, d, (usize)"original", 0, 0, 0, 0), 0);
    expect("unlink remaining hardlink", sc(35, d, (usize)"linked", 0, 0, 0, 0), 0);
    expect("unlink final hardlink while fd stays open", sc(35, d, (usize)"followed", 0, 0, 0, 0), 0);
    expect("seek unlinked open inode", sc(62, fd, 0, 0, 0, 0, 0), 0);
    expect("read unlinked open inode", sc(63, fd, (usize)target, 32, 0, 0, 0), 3);
    expect("unlinked open inode retains contents", target[0], 'o');
    sc(57, fd, 0, 0, 0, 0, 0);
    sc(35, d, (usize)"symlink", 0, 0, 0, 0);
    sc(35, d, (usize)"symlink-copy", 0, 0, 0, 0);
    sc(57, d, 0, 0, 0, 0, 0);
    expect("remove empty directory on ext2", sc(35, -100, (usize)directory, 0x200, 0, 0, 0), 0);
    char *argv[] = {"missing", 0};
    expect("exec missing PATH candidate returns ENOENT", sc(221, (usize)"/usr/bin/scarlet-no-such-executable", (usize)argv, 0, 0, 0, 0), -2);
}

void _start(void)
{
    usize limits[2] = {0, 0};
    expect("prlimit64 RLIMIT_NOFILE", sc(261, 0, 7, 0, (usize)limits, 0, 0), 0);
    expect("FD soft limit matches table capacity", limits[0], 1024);
    expect("FD hard limit matches table capacity", limits[1], 1024);
    expect("prlimit64 bad output pointer", sc(261, 0, 7, 0, 1, 0, 0), -14);

    isize temporary = sc(56, -100, (usize)"/tmp", 0x490082, 0600, 0, 0);
    expect("unsupported O_TMPFILE returns EOPNOTSUPP", temporary, -95);
    if (temporary >= 0) sc(57, temporary, 0, 0, 0, 0, 0);
    expect("O_TMPFILE without O_DIRECTORY is invalid",
           sc(56, -100, (usize)"/tmp", 0x400002, 0600, 0, 0), -22);
    expect("read-only O_TMPFILE is invalid",
           sc(56, -100, (usize)"/tmp", 0x410000, 0600, 0, 0), -22);
    expect("sendmmsg bad fd", sc(269, -1, 0, 0, 0, 0, 0), -9);
    records(2); /* SOCK_DGRAM */
    records(5); /* SOCK_SEQPACKET */
    pipe_readiness();
    file_operations();
    print(failures ? "APT I/O regression FAILED\n" : "APT I/O regression PASSED\n");
    sc(94, failures ? 1 : 0, 0, 0, 0, 0, 0);
    for (;;) {}
}
