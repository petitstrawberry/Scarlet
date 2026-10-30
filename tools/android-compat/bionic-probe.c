/* Run each case in a fresh process: a broken libc startup or thread primitive
 * must not be confused with successful Android application compatibility. */
#define _GNU_SOURCE
#include <dlfcn.h>
#include <errno.h>
#include <dirent.h>
#include <fcntl.h>
#include <linux/stat.h>
#include <linux/futex.h>
#include <pthread.h>
#include <poll.h>
#include <sys/ioctl.h>
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/auxv.h>
#include <sys/mman.h>
#include <sys/resource.h>
#include <sys/signalfd.h>
#include <sys/epoll.h>
#include <sys/stat.h>
#include <sys/xattr.h>
#include <sys/wait.h>
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

extern int information_case(const char *program);
extern int realtime_case(const char *program);
extern int realtime_exec_case(const char *program, const char *mode);
extern int information_exec_case(void);
extern int mount_namespace_case(void);
extern int mount_propagation_case(void);
extern int mount_restrictions_case(const char *program);
extern int process_scope_case(const char *program);
extern int process_scope_exec_case(const char *reaper);
extern int seccomp_case(const char *program);
extern int seccomp_exec_case(void);
extern int credentials_case(const char *program);
extern int credentials_exec_case(void);
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
    // Bionic's main-thread attribute query reads getrlimit, /proc/self/stat
    // field 28, and /proc/self/maps. Exercise that exact path used by ART.
    struct rlimit stack_limit, via_prlimit;
    pthread_attr_t attr;
    void *stack_base;
    size_t stack_size;
    if (getrlimit(RLIMIT_STACK, &stack_limit) ||
        syscall(__NR_prlimit64, getpid(), RLIMIT_STACK, NULL, &via_prlimit) ||
        pthread_getattr_np(pthread_self(), &attr) ||
        pthread_attr_getstack(&attr, &stack_base, &stack_size))
        return fail("libc", "main thread stack query", errno);
    if (stack_limit.rlim_cur != via_prlimit.rlim_cur || stack_limit.rlim_cur != stack_size ||
        stack_limit.rlim_max != stack_size || (uintptr_t)&attr < (uintptr_t)stack_base ||
        (uintptr_t)&attr >= (uintptr_t)stack_base + stack_size)
        return fail("libc", "stack reservation mismatch", 0);
    pthread_attr_destroy(&attr);
    struct rlimit nofile;
    if (getrlimit(RLIMIT_NOFILE, &nofile) || nofile.rlim_cur != (rlim_t)sysconf(_SC_OPEN_MAX))
        return fail("libc", "descriptor limit query", errno);
    if (getrlimit(999, &nofile) != -1 || errno != EINVAL ||
        syscall(__NR_getrlimit, RLIMIT_STACK, NULL) != -1 || errno != EFAULT ||
        syscall(__NR_prlimit64, 0, RLIMIT_STACK, &stack_limit, NULL) != -1 || errno != EOPNOTSUPP)
        return fail("libc", "resource limit validation", errno);
    if (information_case(program)) return 1;
    if (mount_namespace_case()) return 1;
    if (mount_propagation_case()) return 1;
    if (mount_restrictions_case(program)) return 1;
    if (process_scope_case(program)) return 1;
    if (credentials_case(program)) return 1;
    return seccomp_case(program);
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

struct futex_wait_args { uint32_t *word; struct timespec timeout; };
static void *futex_worker(void *argument) {
    struct futex_wait_args *args = argument;
    long rc = syscall(__NR_futex, args->word, FUTEX_WAIT_PRIVATE, 0, &args->timeout, NULL, 0);
    return (void *)(uintptr_t)(rc == 0 ? 0 : errno);
}

static int requeue_threads(void) {
    uint32_t words[3] = {0};
    struct futex_wait_args args = {&words[0], {5, 0}};
    pthread_t threads[2];
#define FCHECK(test) do { if (!(test)) return fail("thread", #test, errno); } while (0)
    for (size_t i = 0; i < 2; ++i) FCHECK(pthread_create(&threads[i], NULL, futex_worker, &args) == 0);
    int moved = 0;
    for (int attempt = 0; moved < 2 && attempt < 3000; ++attempt) {
        long rc = syscall(__NR_futex, &words[0], FUTEX_CMP_REQUEUE_PRIVATE, 0, 1, &words[1], 0);
        FCHECK(rc == 0 || rc == 1);
        moved += (int)rc;
        if (moved < 2) usleep(1000);
    }
    FCHECK(moved == 2);
    FCHECK(syscall(__NR_futex, &words[1], FUTEX_CMP_REQUEUE_PRIVATE, 1, 1, &words[2], 1) == -1 && errno == EAGAIN);
    FCHECK(syscall(__NR_futex, &words[1], FUTEX_REQUEUE_PRIVATE, 1, 1, &words[2], 0) == 2);
    FCHECK(syscall(__NR_futex, &words[0], FUTEX_WAKE_PRIVATE, 2, NULL, NULL, 0) == 0);
    FCHECK(syscall(__NR_futex, &words[1], FUTEX_WAKE_PRIVATE, 2, NULL, NULL, 0) == 0);
    FCHECK(syscall(__NR_futex, &words[2], FUTEX_WAKE_PRIVATE, 2, NULL, NULL, 0) == 1);
    for (size_t i = 0; i < 2; ++i) {
        void *result;
        FCHECK(pthread_join(threads[i], &result) == 0 && result == NULL);
    }
    // The timer was armed against words[0]; after requeue it must remove the
    // registration from words[2], without leaving a phantom wake there.
    args.timeout = (struct timespec){0, 500000000};
    FCHECK(pthread_create(&threads[0], NULL, futex_worker, &args) == 0);
    moved = 0;
    for (int attempt = 0; !moved && attempt < 300; ++attempt) {
        moved = (int)syscall(__NR_futex, &words[0], FUTEX_REQUEUE_PRIVATE, 0, 1, &words[2], 0);
        FCHECK(moved == 0 || moved == 1);
        if (!moved) usleep(1000);
    }
    FCHECK(moved == 1);
    void *result;
    FCHECK(pthread_join(threads[0], &result) == 0 && (uintptr_t)result == ETIMEDOUT);
    FCHECK(syscall(__NR_futex, &words[2], FUTEX_WAKE_PRIVATE, 2, NULL, NULL, 0) == 0);
#undef FCHECK
    return 0;
}

struct thread_directory_wait { int ready; int release; };
static void *directory_worker(void *argument) {
    struct thread_directory_wait *fds = argument;
    pid_t tid = gettid(); char byte;
    return (void *)(uintptr_t)(write(fds->ready, &tid, sizeof(tid)) != sizeof(tid) || read(fds->release, &byte, 1) != 1);
}
static int thread_directory_count(pid_t expected_peer) {
    DIR *dir = opendir("/proc/self/task");
    if (!dir) return fail("thread", "task directory open", errno);
    int own = 0, peer = 0, bad = 0;
    struct dirent *entry;
    while ((entry = readdir(dir))) {
        if (!strcmp(entry->d_name, ".") || !strcmp(entry->d_name, "..")) continue;
        pid_t tid = (pid_t)strtol(entry->d_name, NULL, 10);
        if (tid == gettid()) ++own;
        else if (expected_peer && tid == expected_peer) ++peer;
        else bad = 1;
        if (entry->d_type != DT_DIR) bad = 1;
    }
    closedir(dir);
    if (bad || own != 1 || peer != !!expected_peer) return fail("thread", "live thread enumeration", 0);
    return 0;
}
static int thread_directory_case(void) {
    int ready[2], release[2]; pthread_t thread;
    if (pipe(ready) || pipe(release)) return fail("thread", "task directory pipes", errno);
    struct thread_directory_wait args = {ready[1], release[0]};
    int rc = pthread_create(&thread, NULL, directory_worker, &args);
    if (rc) return fail("thread", "task directory worker", rc);
    pid_t tid;
    if (read(ready[0], &tid, sizeof(tid)) != sizeof(tid)) return fail("thread", "task directory ready", errno);
    int result = thread_directory_count(tid);
    if (write(release[1], "x", 1) != 1) return fail("thread", "task directory release", errno);
    void *status; rc = pthread_join(thread, &status);
    close(ready[0]); close(ready[1]); close(release[0]); close(release[1]);
    if (result || rc || status) return fail("thread", "task directory join", rc);
    return thread_directory_count(0);
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
    if (thread_directory_case()) return 1;
    return requeue_threads();
}

static void receive_signal(int number) { signal_seen = number; }

struct signal_wait_args { int ready; sigset_t mask; int interrupt; };
static void *signal_wait_worker(void *argument) {
    struct signal_wait_args *args = argument;
    if (write(args->ready, "r", 1) != 1) return (void *)1;
    int number = 0;
    if (args->interrupt) {
        struct timespec timeout = {2, 0};
        int rc = sigtimedwait(&args->mask, NULL, &timeout);
        return (void *)(uintptr_t)(rc == -1 && errno == EINTR ? 0 : 2);
    }
    int rc = sigwait(&args->mask, &number);
    return (void *)(uintptr_t)(rc == 0 && number == SIGUSR1 ? 0 : 3);
}

static int signal_case(void) {
    struct sigaction action = {0};
    action.sa_handler = receive_signal;
    sigemptyset(&action.sa_mask);
    if (sigaction(SIGUSR1, &action, NULL)) return fail("signal", "sigaction", errno);
    int rc = raise(SIGUSR1);
    if (rc) return fail("signal", "raise", errno);
    if (signal_seen != SIGUSR1) return fail("signal", "handler was not called", 0);
    sigset_t mask, original;
    sigemptyset(&mask);
    sigaddset(&mask, SIGUSR1);
#define SCHECK(test) do { if (!(test)) return fail("signal", #test, errno); } while (0)
    SCHECK(pthread_sigmask(SIG_BLOCK, &mask, &original) == 0);
    signal_seen = 0;
    SCHECK(raise(SIGUSR1) == 0);
    siginfo_t info;
    struct timespec zero = {0}, short_timeout = {0, 20000000};
    SCHECK(sigtimedwait(&mask, &info, &zero) == SIGUSR1 && info.si_signo == SIGUSR1);
    SCHECK(signal_seen == 0);
    SCHECK(sigtimedwait(&mask, NULL, &zero) == -1 && errno == EAGAIN);
    SCHECK(sigtimedwait(&mask, NULL, &short_timeout) == -1 && errno == EAGAIN);
    SCHECK(sigaction(SIGINT, &action, NULL) == 0);
    for (int interrupt = 0; interrupt < 2; ++interrupt) {
        int pipefd[2];
        SCHECK(pipe(pipefd) == 0);
        struct signal_wait_args args = {pipefd[1], mask, interrupt};
        pthread_t waiter;
        SCHECK(pthread_create(&waiter, NULL, signal_wait_worker, &args) == 0);
        char ready;
        SCHECK(read(pipefd[0], &ready, 1) == 1);
        usleep(20000);
        SCHECK(pthread_kill(waiter, interrupt ? SIGINT : SIGUSR1) == 0);
        void *result;
        SCHECK(pthread_join(waiter, &result) == 0 && result == NULL);
        SCHECK(close(pipefd[0]) == 0 && close(pipefd[1]) == 0);
    }
    SCHECK(signal_seen == SIGINT);
    sigset_t children;sigemptyset(&children);sigaddset(&children,SIGCHLD);
    SCHECK(pthread_sigmask(SIG_BLOCK,&children,NULL)==0);
    int signal_fd=signalfd(-1,&children,SFD_NONBLOCK|SFD_CLOEXEC), poll_fd=epoll_create1(EPOLL_CLOEXEC);
    SCHECK(signal_fd>=0 && poll_fd>=0);
    struct epoll_event event={.events=EPOLLIN,.data.u64=17};
    SCHECK(epoll_ctl(poll_fd,EPOLL_CTL_ADD,signal_fd,&event)==0);
    pid_t child=fork();SCHECK(child>=0);if(!child)_exit(23);
    int ready;do {ready=epoll_wait(poll_fd,&event,1,2000);}while(ready<0 && errno==EINTR);
    SCHECK(ready==1 && event.data.u64==17 && (event.events&EPOLLIN));
    struct signalfd_siginfo child_info;
    SCHECK(read(signal_fd,&child_info,sizeof(child_info))==sizeof(child_info) && child_info.ssi_signo==SIGCHLD);
    int child_status;SCHECK(waitpid(child,&child_status,0)==child && WIFEXITED(child_status) && WEXITSTATUS(child_status)==23);
    SCHECK(read(signal_fd,&child_info,sizeof(child_info))==-1 && errno==EAGAIN);
    SCHECK(close(signal_fd)==0 && close(poll_fd)==0);
    printf("SCARLET_CHILD_EXIT_SIGNALFD_OK epoll waitpid\n");
    int master=posix_openpt(O_RDWR|O_NOCTTY|O_CLOEXEC);
    SCHECK(master>=0 && grantpt(master)==0 && unlockpt(master)==0);
    char slave_path[128];SCHECK(ptsname_r(master,slave_path,sizeof(slave_path))==0);
    int slave=open(slave_path,O_RDWR|O_NOCTTY|O_CLOEXEC);SCHECK(slave>=0);
    struct pollfd terminal={.fd=master,.events=POLLIN};
    SCHECK(poll(&terminal,1,0)==0);
    SCHECK(write(slave,"x",1)==1);
    SCHECK(close(slave)==0);
    SCHECK(poll(&terminal,1,1000)==1 && (terminal.revents&POLLHUP));
    char byte;SCHECK(read(master,&byte,1)==1 && byte=='x');
    terminal.events=0;SCHECK(poll(&terminal,1,0)==1 && (terminal.revents&POLLHUP));
    int terminal_epoll=epoll_create1(EPOLL_CLOEXEC);SCHECK(terminal_epoll>=0);
    event.events=EPOLLIN;event.data.u64=18;
    SCHECK(epoll_ctl(terminal_epoll,EPOLL_CTL_ADD,master,&event)==0);
    SCHECK(epoll_wait(terminal_epoll,&event,1,1000)==1 && (event.events&EPOLLHUP));
    SCHECK(close(terminal_epoll)==0 && close(master)==0);
    printf("SCARLET_PTY_HANGUP_OK poll epoll buffered-output\n");

    SCHECK(pthread_sigmask(SIG_SETMASK, &original, NULL) == 0);
#undef SCHECK
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

extern int __cxa_atexit(void (*function)(void *), void *argument, void *dso);
extern void __cxa_finalize(void *dso);
static void count_exit_handler(void *argument) { ++*(size_t *)argument; }

static int mremap_case(void) {
    size_t page = (size_t)sysconf(_SC_PAGESIZE), size = page * 3;
    unsigned char *source = mmap(NULL, size, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    unsigned char *reservation = mmap(NULL, page * 5, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
#define MCHECK(test) do { if (!(test)) return fail("mremap", #test, errno); } while (0)
    MCHECK(source != MAP_FAILED && reservation != MAP_FAILED);
    source[0] = 42; source[page * 2] = 77;
    reservation[0] = 11; reservation[page * 4] = 22;
    reservation[page] = 99;
    MCHECK(mprotect(source, size, PROT_READ) == 0);
    int sync[2];
    MCHECK(pipe(sync) == 0);
    pid_t child = fork();
    MCHECK(child >= 0);
    if (child == 0) {
        close(sync[1]);
        char ready;
        if (read(sync[0], &ready, 1) != 1 || source[0] != 42 || source[page] != 0 || source[2 * page] != 77)
            _exit(31);
        _exit(0);
    }
    close(sync[0]);
    unsigned char *destination = reservation + page;
    MCHECK(mremap(source, size, size, MREMAP_MAYMOVE | MREMAP_FIXED, destination) == destination);
    MCHECK(destination[0] == 42 && destination[page] == 0 && destination[2 * page] == 77);
    MCHECK(reservation[0] == 11 && reservation[page * 4] == 22);
    MCHECK(mremap(destination, size, size, MREMAP_MAYMOVE | MREMAP_FIXED, destination + page) == MAP_FAILED && errno == EINVAL);
    MCHECK(mprotect(destination, size, PROT_READ | PROT_WRITE) == 0);
    destination[0] = 55;
    MCHECK(mmap(source, size, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED_NOREPLACE, -1, 0) == source);
    source[0] = 66;
    MCHECK(destination[0] == 55);
    MCHECK(write(sync[1], "x", 1) == 1 && close(sync[1]) == 0);
    int status;
    MCHECK(waitpid(child, &status, 0) == child && WIFEXITED(status) && WEXITSTATUS(status) == 0);
    MCHECK(munmap(source, size) == 0 && munmap(reservation, 5 * page) == 0);
    source = mmap(NULL, page, PROT_NONE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    destination = mmap(NULL, page, PROT_NONE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    MCHECK(source != MAP_FAILED && destination != MAP_FAILED);
    MCHECK(mremap(source, page, page, MREMAP_MAYMOVE | MREMAP_FIXED, destination) == destination);
    MCHECK(mprotect(destination, page, PROT_READ | PROT_WRITE) == 0);
    MCHECK(destination[0] == 0);
    destination[0] = 88;
    MCHECK(munmap(destination, page) == 0);
    // Reserve a known gap and neighbor so both in-place and moving growth are
    // deterministic, independently of the process's other mappings.
    reservation = mmap(NULL, 5 * page, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    MCHECK(reservation != MAP_FAILED);
    reservation[0] = 42; reservation[page] = 77; reservation[4 * page] = 99;
    MCHECK(munmap(reservation + 2 * page, 2 * page) == 0);
    MCHECK(mprotect(reservation, 2 * page, PROT_READ) == 0);
    MCHECK(mremap(reservation, 2 * page, 3 * page, 0) == reservation);
    MCHECK(reservation[0] == 42 && reservation[page] == 77 && reservation[2 * page] == 0);
    MCHECK(mremap(reservation, 3 * page, 5 * page, 0) == MAP_FAILED && errno == ENOMEM);
    destination = mremap(reservation, 3 * page, 5 * page, MREMAP_MAYMOVE);
    MCHECK(destination != MAP_FAILED && destination != reservation);
    MCHECK(destination[0] == 42 && destination[page] == 77 && destination[2 * page] == 0 &&
           destination[3 * page] == 0 && destination[4 * page] == 0 && reservation[4 * page] == 99);
    MCHECK(mprotect(destination, 5 * page, PROT_READ | PROT_WRITE) == 0);
    destination[4 * page] = 88;
    MCHECK(mremap(destination, 5 * page, 3 * page, 0) == destination);
    MCHECK(mremap(destination, 3 * page, 5 * page, 0) == destination);
    MCHECK(destination[4 * page] == 0);
    MCHECK(munmap(destination, 5 * page) == 0 && munmap(reservation + 4 * page, page) == 0);
    // Exercise Bionic's actual atexit array expansion and mprotect splits, the
    // path used while libart and its dependencies register their destructors.
    size_t calls = 0;
    for (size_t i = 0; i < 512; ++i)
        MCHECK(__cxa_atexit(count_exit_handler, &calls, &calls) == 0);
    __cxa_finalize(&calls);
    MCHECK(calls == 512);
#undef MCHECK
    return 0;
}

static int xattr_case(void) {
    char path[96], link[100], renamed[100], bytes[128];
    snprintf(path, sizeof(path), "/tmp/bionic-xattr-%d", getpid());
    snprintf(link, sizeof(link), "%s-ln", path);
    snprintf(renamed, sizeof(renamed), "%s-new", path);
    int fd = open(path, O_RDWR | O_CREAT | O_EXCL, 0600);
    const char value[] = {0, 1, 127, (char)255};
#define XCHECK(test) do { if (!(test)) return fail("xattr", #test, errno); } while (0)
    XCHECK(fd >= 0);
    XCHECK(fgetxattr(fd, "user.test", NULL, 0) == -1 && errno == ENODATA);
    XCHECK(fsetxattr(fd, "user.test", value, sizeof(value), XATTR_CREATE) == 0);
    XCHECK(fsetxattr(fd, "user.test", "wrong", 5, XATTR_CREATE) == -1 && errno == EEXIST);
    XCHECK(fsetxattr(fd, "user.absent", "", 0, XATTR_REPLACE) == -1 && errno == ENODATA);
    XCHECK(fsetxattr(fd, "user.test", "", 0, 3) == -1 && errno == EINVAL);
    XCHECK(fsetxattr(fd, "user.test", (void *)1, 65537, 0) == -1 && errno == E2BIG);
    XCHECK(fsetxattr(fd, "user.test", (void *)1, 4, 0) == -1 && errno == EFAULT);
    XCHECK(getxattr(path, "user.test", NULL, 0) == sizeof(value));
    memset(bytes, 'x', sizeof(bytes));
    XCHECK(fgetxattr(fd, "user.test", bytes, 3) == -1 && errno == ERANGE && bytes[0] == 'x');
    XCHECK(fgetxattr(fd, "user.test", bytes, sizeof(bytes)) == sizeof(value) && !memcmp(bytes, value, sizeof(value)));
    XCHECK(flistxattr(fd, NULL, 0) == sizeof("user.test"));
    XCHECK(listxattr(path, bytes, sizeof(bytes)) == sizeof("user.test") && !strcmp(bytes, "user.test"));
    XCHECK(symlink(path, link) == 0);
    XCHECK(getxattr(link, "user.test", bytes, sizeof(bytes)) == sizeof(value));
    XCHECK(lgetxattr(link, "user.test", bytes, sizeof(bytes)) == -1 && errno == ENODATA);
    XCHECK(lsetxattr(link, "user.test", "x", 1, 0) == -1 && errno == EPERM);
    XCHECK(lsetxattr(link, "security.selinux", "label", 6, XATTR_CREATE) == 0);
    XCHECK(lgetxattr(link, "security.selinux", bytes, sizeof(bytes)) == 6 && !strcmp(bytes, "label"));
    XCHECK(llistxattr(link, bytes, sizeof(bytes)) == sizeof("security.selinux"));
    XCHECK(lremovexattr(link, "security.selinux") == 0);
    XCHECK(rename(path, renamed) == 0);
    XCHECK(getxattr(renamed, "user.test", bytes, sizeof(bytes)) == sizeof(value));
    XCHECK(unlink(renamed) == 0);
    XCHECK(fgetxattr(fd, "user.test", bytes, sizeof(bytes)) == sizeof(value));
    XCHECK(fsetxattr(fd, "user.test", "", 0, XATTR_REPLACE) == 0);
    XCHECK(fgetxattr(fd, "user.test", NULL, 0) == 0);
    XCHECK(fremovexattr(fd, "user.test") == 0);
    XCHECK(flistxattr(fd, bytes, sizeof(bytes)) == 0);
    XCHECK(fremovexattr(fd, "user.test") == -1 && errno == ENODATA);
    XCHECK(fgetxattr(-1, "user.test", bytes, sizeof(bytes)) == -1 && errno == EBADF);
    XCHECK(unlink(link) == 0 && close(fd) == 0);
    XCHECK(getxattr("/bin/bionic-static", "user.test", NULL, 0) == -1 && errno == EOPNOTSUPP);
#undef XCHECK
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
    if (argc == 2 && (!strcmp(argv[1], "realtime-ambient") || !strcmp(argv[1], "realtime-stripped"))) return realtime_exec_case(argv[0], argv[1]);
    if (argc == 3 && !strcmp(argv[1], "scope-orphan-exec")) return process_scope_exec_case(argv[2]);
    if (argc == 2 && !strcmp(argv[1], "information-exec")) return information_exec_case();
    if (argc == 2 && !strcmp(argv[1], "seccomp-exec")) return seccomp_exec_case();
    if (argc == 2 && !strcmp(argv[1], "identity-exec")) return credentials_exec_case();
    if (argc != 2) return fail("arguments", "expected one case name", 0);
    const char *case_name = !strcmp(argv[1], "exec-child") ? "exec" : argv[1];
    if (printf("SCARLET_BIONIC_BEGIN %s\n", case_name) < 0 || fflush(stdout)) return 2;
    int rc;
    if (!strcmp(argv[1], "libc")) rc = libc_case(argv[0]);
    else if (!strcmp(argv[1], "thread")) rc = thread_case();
    else if (!strcmp(argv[1], "signal")) rc = signal_case();
    else if (!strcmp(argv[1], "shared")) rc = shared_case();
    else if (!strcmp(argv[1], "dynamic")) rc = dynamic_case();
    else if (!strcmp(argv[1], "mremap")) rc = mremap_case();
    else if (!strcmp(argv[1], "xattr")) rc = xattr_case();
    else if (!strcmp(argv[1], "binder")) rc = binder_case();
    else if (!strcmp(argv[1], "realtime")) rc = realtime_case(argv[0]);
    else if (!strcmp(argv[1], "exec")) rc = exec_case(argv[0]);
    else if (!strcmp(argv[1], "exec-child")) rc = exec_child_case();
    else if (!strcmp(argv[1], "binder-absent")) rc = binder_absent_case();
    else return fail("arguments", "unknown case", 0);
    if (!rc && (printf("SCARLET_BIONIC_OK %s\n", case_name) < 0 || fflush(stdout))) return 2;
    return rc;
}
