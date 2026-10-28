/* Run each case in a fresh process: a broken libc startup or thread primitive
 * must not be confused with successful Android application compatibility. */
#define _GNU_SOURCE
#include <dlfcn.h>
#include <errno.h>
#include <fcntl.h>
#include <linux/stat.h>
#include <pthread.h>
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/auxv.h>
#include <sys/mman.h>
#include <sys/stat.h>
#include <sys/syscall.h>
#include <unistd.h>

#ifndef __BIONIC__
#error Build this probe with the Android NDK, not a Linux libc.
#endif

static int initialized;
int binder_case(void);
static __thread int tls_value = 17;
static volatile sig_atomic_t signal_seen;
static pthread_mutex_t mutex = PTHREAD_MUTEX_INITIALIZER;
static int shared_value;

__attribute__((constructor)) static void initialize(void) { initialized = 42; }

static int fail(const char *case_name, const char *operation, int error) {
    printf("SCARLET_BIONIC_FAIL %s: %s (%d)\n", case_name, operation, error);
    return 1;
}

static int libc_case(const char *program) {
    if (initialized != 42 || tls_value != 17)
        return fail("libc", "constructor/TLS initialization", 0);
    if (getauxval(AT_PAGESZ) != (unsigned long)sysconf(_SC_PAGESIZE) ||
        !getauxval(AT_RANDOM) || !getauxval(AT_PHDR))
        return fail("libc", "auxiliary vector", 0);
    struct stat executable, direct, link;
    struct statx extended;
    if (stat("/proc/self/exe", &executable) || stat(program, &direct) ||
        lstat("/proc/self/exe", &link) ||
        syscall(__NR_statx, AT_FDCWD, "/proc/self/exe", 0, STATX_BASIC_STATS, &extended))
        return fail("libc", "executable stat/lstat/statx", errno);
    if (!S_ISLNK(link.st_mode) || !S_ISREG(executable.st_mode) ||
        executable.st_ino != direct.st_ino || executable.st_size != direct.st_size ||
        extended.stx_ino != executable.st_ino || extended.stx_size != (uint64_t)executable.st_size)
        return fail("libc", "executable metadata mismatch", 0);
    char *p = malloc(8192);
    if (!p) return fail("libc", "malloc", errno);
    memset(p, 0x5a, 8192);
    if (p[8191] != 0x5a) return fail("libc", "allocation contents", 0);
    free(p);
    unsigned char *mapping = mmap(NULL, 8192, PROT_READ | PROT_WRITE,
                                  MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (mapping == MAP_FAILED) return fail("libc", "tagged access mmap", errno);
    // AArch64 Linux guarantees TBI for userspace loads/stores independently of
    // PR_TAGGED_ADDR_ENABLE. Keep syscall pointers untagged.
    volatile unsigned char *tagged = (void *)((uintptr_t)mapping | (UINT64_C(2) << 56));
    tagged[4109] = 91;
    if (mapping[4109] != 91) return fail("libc", "tagged access alias", 0);
    if (munmap(mapping, 8192)) return fail("libc", "tagged access munmap", errno);
    return 0;
}

static void *worker(void *unused) {
    (void)unused;
    if (tls_value != 17) return (void *)(uintptr_t)1;
    tls_value = 29;
    int rc = pthread_mutex_lock(&mutex);
    if (rc) return (void *)(uintptr_t)rc;
    shared_value = 73;
    rc = pthread_mutex_unlock(&mutex);
    return (void *)(uintptr_t)rc;
}

static int thread_case(void) {
    pthread_t thread;
    tls_value = 23;
    int rc = pthread_create(&thread, NULL, worker, NULL);
    if (rc) return fail("thread", "pthread_create", rc);
    void *result = NULL;
    rc = pthread_join(thread, &result);
    if (rc || result) return fail("thread", "pthread_join/worker", rc ? rc : (int)(uintptr_t)result);
    if (tls_value != 23 || shared_value != 73)
        return fail("thread", "TLS isolation/shared address space", 0);
    return 0;
}

static void receive_signal(int number) { signal_seen = number; }

static int signal_case(void) {
    struct sigaction action = {0};
    action.sa_handler = receive_signal;
    sigemptyset(&action.sa_mask);
    if (sigaction(SIGUSR1, &action, NULL)) return fail("signal", "sigaction", errno);
    int rc = raise(SIGUSR1);
    if (rc) return fail("signal", "raise", errno);
    if (signal_seen != SIGUSR1) return fail("signal", "handler was not called", 0);
    return 0;
}

static int shared_case(void) {
    int fd = (int)syscall(__NR_memfd_create, "scarlet-bionic", 0);
    if (fd < 0) return fail("shared", "memfd_create", errno);
    if (ftruncate(fd, 4096)) return fail("shared", "ftruncate", errno);
    unsigned char *a = mmap(NULL, 4096, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
    unsigned char *b = mmap(NULL, 4096, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
    if (a == MAP_FAILED || b == MAP_FAILED) return fail("shared", "mmap", errno);
    a[13] = 73;
    b[4095] = 91;
    if (b[13] != 73 || a[4095] != 91) return fail("shared", "shared mappings disagree", 0);
    if (munmap(a, 4096) || munmap(b, 4096) || close(fd))
        return fail("shared", "cleanup", errno);
    return 0;
}

static int dynamic_case(void) {
    void *library = dlopen("libbionic-probe.so", RTLD_NOW | RTLD_LOCAL);
    if (!library) {
        printf("SCARLET_BIONIC_FAIL dynamic: dlopen: %s\n", dlerror());
        return 1;
    }
    int (*value)(void) = (int (*)(void))dlsym(library, "bionic_probe_value");
    if (!value || value() != 101) return fail("dynamic", "DSO constructor/TLS/symbol", 0);
    return dlclose(library) ? fail("dynamic", "dlclose", 0) : 0;
}

static int exec_case(const char *program) {
    int original = (int)syscall(__NR_memfd_create, "android-exec", 0);
    if (original < 0) return fail("exec", "memfd_create", errno);
    int retained = fcntl(original, F_DUPFD, 37);
    int closed = fcntl(original, F_DUPFD_CLOEXEC, 38);
    if (retained != 37 || closed != 38 || write(retained, "exec-ok", 7) != 7 ||
        lseek(retained, 0, SEEK_SET) != 0 || fcntl(retained, F_SETFL, O_NONBLOCK))
        return fail("exec", "prepare sparse descriptor table", errno);
    close(original);
    char *args[] = {(char *)program, "exec-child", NULL};
    execve(program, args, environ);
    return fail("exec", "execve", errno);
}

static int exec_child_case(void) {
    char bytes[7];
    if (!(fcntl(37, F_GETFL) & O_NONBLOCK) || fcntl(38, F_GETFD) != -1 || errno != EBADF ||
        read(37, bytes, sizeof(bytes)) != sizeof(bytes) || memcmp(bytes, "exec-ok", sizeof(bytes)))
        return fail("exec", "descriptor state across composed ABI exec", errno);
    // Exec must retain the outer Android ABI and its device translations.
    int binder = open("/dev/binder", O_RDWR | O_CLOEXEC);
    if (binder < 0) return fail("exec", "Android device after exec", errno);
    close(binder);
    close(37);
    return 0;
}

static int binder_absent_case(void) {
    for (const char **path = (const char *[]){"/dev/binder", "/dev/hwbinder", "/dev/vndbinder", NULL};
         *path; ++path) {
        int fd = open(*path, O_RDWR | O_CLOEXEC);
        if (fd >= 0 || errno != ENOENT)
            return fail("binder-absent", "Android device exposed to plain Linux ABI", errno);
    }
    return 0;
}

int main(int argc, char **argv) {
    setvbuf(stdout, NULL, _IONBF, 0);
    if (argc != 2) return fail("arguments", "expected one case name", 0);
    const char *case_name = !strcmp(argv[1], "exec-child") ? "exec" : argv[1];
    if (printf("SCARLET_BIONIC_BEGIN %s\n", case_name) < 0 || fflush(stdout)) return 2;
    int rc;
    if (!strcmp(argv[1], "libc")) rc = libc_case(argv[0]);
    else if (!strcmp(argv[1], "thread")) rc = thread_case();
    else if (!strcmp(argv[1], "signal")) rc = signal_case();
    else if (!strcmp(argv[1], "shared")) rc = shared_case();
    else if (!strcmp(argv[1], "dynamic")) rc = dynamic_case();
    else if (!strcmp(argv[1], "binder")) rc = binder_case();
    else if (!strcmp(argv[1], "exec")) rc = exec_case(argv[0]);
    else if (!strcmp(argv[1], "exec-child")) rc = exec_child_case();
    else if (!strcmp(argv[1], "binder-absent")) rc = binder_absent_case();
    else return fail("arguments", "unknown case", 0);
    if (!rc && (printf("SCARLET_BIONIC_OK %s\n", case_name) < 0 || fflush(stdout))) return 2;
    return rc;
}
