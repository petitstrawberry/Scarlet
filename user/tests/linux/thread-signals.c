#define _GNU_SOURCE
#include <errno.h>
#include <pthread.h>
#include <signal.h>
#include <stdatomic.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/syscall.h>
#include <time.h>
#include <unistd.h>

/* Ordinary musl pthreads exercise the libc setxid broadcast, not a mock. */
static pid_t worker_tids[2];
static _Atomic int ready, stop;
static int failures, checks, unavailable;
static volatile sig_atomic_t handled;

static void check(int ok, const char *name)
{
    ++checks;
    if (!ok) ++failures;
    printf("%s: %s\n", ok ? "PASS" : "FAIL", name);
}

static void handler(int signal)
{
    (void)signal;
    ++handled;
}

static void *worker(void *argument)
{
    size_t index = (size_t)argument;
    worker_tids[index] = (pid_t)syscall(SYS_gettid);
    atomic_fetch_add_explicit(&ready, 1, memory_order_release);
    while (!atomic_load_explicit(&stop, memory_order_acquire)) {
        struct timespec pause = { .tv_nsec = 1000000 };
        nanosleep(&pause, NULL);
    }
    return NULL;
}

static void expect_signal(long result, int error, const char *name)
{
    check(unavailable ? result == -1 && error == ENOSYS : result == 0,
          name);
}

int main(int argc, char **argv)
{
    unavailable = argc == 2 && !strcmp(argv[1], "--expect-unavailable");
    if (argc > 1 && !unavailable) return 2;
    setbuf(stdout, NULL);
    struct sigaction action = { .sa_handler = handler };
    sigemptyset(&action.sa_mask);
    check(sigaction(SIGUSR1, &action, NULL) == 0, "install custom handler");

    pid_t pid = getpid(), tid = (pid_t)syscall(SYS_gettid);
    check(syscall(SYS_tkill, tid, 0) == 0, "tkill signal-zero probe");
    check(syscall(SYS_tgkill, pid, tid, 0) == 0, "tgkill signal-zero probe");
    errno = 0;
    long result = syscall(SYS_tkill, 0x7fffffff, 0);
    check(result == -1 && errno == ESRCH, "unknown TID returns ESRCH");
    errno = 0;
    result = syscall(SYS_tgkill, pid, tid, 65);
    check(result == -1 && errno == EINVAL, "invalid signal returns EINVAL");

    errno = 0;
    result = syscall(SYS_tkill, tid, SIGUSR1);
    expect_signal(result, errno, "tkill custom handler result");
    errno = 0;
    result = syscall(SYS_tgkill, pid, tid, SIGUSR1);
    expect_signal(result, errno, "tgkill custom handler result");
    if (!unavailable) check(handled == 2, "Linux executes both self handlers");

    sigset_t mask;
    sigemptyset(&mask);
    sigaddset(&mask, SIGUSR1);
    check(pthread_sigmask(SIG_BLOCK, &mask, NULL) == 0, "block custom signal");
    errno = 0;
    result = syscall(SYS_tkill, tid, SIGUSR1);
    expect_signal(result, errno, "masked custom signal result");
    check(pthread_sigmask(SIG_UNBLOCK, &mask, NULL) == 0, "unblock custom signal");
    if (unavailable) check(handled == 0, "failed sends do not invoke a handler");

    pthread_t threads[2];
    for (size_t index = 0; index < 2; ++index) {
        int error = pthread_create(&threads[index], NULL, worker, (void *)index);
        if (error) {
            fprintf(stderr, "pthread_create: %s\n", strerror(error));
            return 1;
        }
    }
    while (atomic_load_explicit(&ready, memory_order_acquire) != 2) {
        struct timespec pause = { .tv_nsec = 1000000 };
        nanosleep(&pause, NULL);
    }
    check(worker_tids[0] != tid && worker_tids[1] != tid &&
          worker_tids[0] != worker_tids[1], "two live pthread TIDs");
    errno = 0;
    result = syscall(SYS_tkill, worker_tids[0], SIGUSR1);
    expect_signal(result, errno, "tkill custom handler on another pthread");
    errno = 0;
    result = syscall(SYS_tgkill, pid, worker_tids[1], SIGUSR1);
    expect_signal(result, errno, "tgkill custom handler on another pthread");
    check(syscall(SYS_tkill, worker_tids[0], SIGWINCH) == 0,
          "default ignored signal remains supported");

    uid_t uid = getuid(), euid = geteuid();
    gid_t gid = getgid(), egid = getegid();
    puts("Checking libc setreuid broadcast with two live pthreads...");
    errno = 0;
    int rc = setreuid(uid, uid), error = errno;
    printf("setreuid returned %d (errno %d)\n", rc, error);
    check(unavailable ? rc == -1 && error != 0 : rc == 0,
          "libc setreuid broadcast returns without hanging");
    puts("Checking libc setegid broadcast with two live pthreads...");
    errno = 0;
    rc = setegid(gid);
    error = errno;
    printf("setegid returned %d (errno %d)\n", rc, error);
    check(unavailable ? rc == -1 && error != 0 : rc == 0,
          "libc setegid broadcast returns without hanging");
    check(getuid() == uid && geteuid() == euid &&
          getgid() == gid && getegid() == egid,
          "same-identity calls leave credentials unchanged");

    atomic_store_explicit(&stop, 1, memory_order_release);
    check(pthread_join(threads[0], NULL) == 0 &&
          pthread_join(threads[1], NULL) == 0, "both pthreads remain joinable");
    printf("%s: %d thread-signal checks (%s)\n", failures ? "FAIL" : "PASS",
           checks, unavailable ? "unsupported handlers" : "Linux baseline");
    return failures != 0;
}
