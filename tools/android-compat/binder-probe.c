/* Real NDK Binder UAPI, two processes, no modified Android headers/runtime. */
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <poll.h>
#include <pthread.h>
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/epoll.h>
#include <sys/mman.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <unistd.h>
#include <linux/android/binder.h>

#define REQUIRE(x) do { if (!(x)) { printf("SCARLET_BIONIC_FAIL binder: line %d: %s errno=%d\n", __LINE__, #x, errno); return 1; } } while (0)
#define POOL_SIZE (1024 * 1024 - 8192)
struct peer { int fd; void *pool; };

static int open_peer(struct peer *peer) {
    peer->fd = open("/dev/binder", O_RDWR | O_CLOEXEC);
    REQUIRE(peer->fd >= 0);
    struct binder_version version = {0};
    REQUIRE(ioctl(peer->fd, BINDER_VERSION, &version) == 0 && version.protocol_version == 8);
    REQUIRE(mmap(NULL, POOL_SIZE, PROT_READ | PROT_WRITE, MAP_PRIVATE, peer->fd, 0) == MAP_FAILED);
    peer->pool = mmap(NULL, POOL_SIZE, PROT_READ, MAP_PRIVATE | MAP_NORESERVE, peer->fd, 0);
    REQUIRE(peer->pool != MAP_FAILED);
    const volatile unsigned char *bytes = peer->pool;
    REQUIRE(bytes[0] == 0 && bytes[POOL_SIZE - 1] == 0);
    REQUIRE(mprotect(peer->pool, POOL_SIZE, PROT_READ | PROT_WRITE) == -1);
    return 0;
}
static int send_command(int fd, uint32_t command, const void *data, size_t size) {
    unsigned char bytes[4 + sizeof(struct binder_transaction_data)];
    memcpy(bytes, &command, 4);
    if (size) memcpy(bytes + 4, data, size);
    struct binder_write_read bwr = {.write_size = 4 + size, .write_buffer = (uintptr_t)bytes};
    if (ioctl(fd, BINDER_WRITE_READ, &bwr)) return -1;
    if (bwr.write_consumed != 4 + size) { errno = EIO; return -1; }
    return 0;
}
static int receive(int fd, uint32_t expected, struct binder_transaction_data *tx) {
    for (int i = 0; i < 8; ++i) {
        unsigned char bytes[256] = {0};
        struct binder_write_read bwr = {.read_size = sizeof(bytes), .read_buffer = (uintptr_t)bytes};
        REQUIRE(ioctl(fd, BINDER_WRITE_READ, &bwr) == 0);
        REQUIRE(bwr.read_consumed >= 4);
        uint32_t command;
        memcpy(&command, bytes, 4);
        if ((command == BR_TRANSACTION_COMPLETE || command == BR_NOOP) && command != expected) continue;
        REQUIRE(command == expected);
        if (tx) {
            REQUIRE(bwr.read_consumed == 4 + sizeof(*tx));
            memcpy(tx, bytes + 4, sizeof(*tx));
        }
        return 0;
    }
    REQUIRE(0);
}
static int free_buffer(int fd, const struct binder_transaction_data *tx) {
    binder_uintptr_t ptr = tx->data.ptr.buffer;
    return send_command(fd, BC_FREE_BUFFER, &ptr, sizeof(ptr));
}
static int poll_readable(int fd, int timeout, int expected) {
    struct pollfd pfd = {.fd = fd, .events = POLLIN};
    REQUIRE(poll(&pfd, 1, timeout) == expected);
    REQUIRE(pfd.revents == (expected ? POLLIN : 0));
    return 0;
}
static int epoll_level(int fd) {
    int epfd = epoll_create1(EPOLL_CLOEXEC);
    REQUIRE(epfd >= 0);
    struct epoll_event event = {.events = EPOLLIN, .data.u64 = 0x123456789abcdef0ULL};
    REQUIRE(epoll_ctl(epfd, EPOLL_CTL_ADD, fd, &event) == 0);
    // Readiness must persist across observations until BINDER_WRITE_READ
    // consumes the pending command; polling must not consume RPC data.
    for (int i = 0; i < 2; ++i) {
        struct epoll_event result = {0};
        REQUIRE(epoll_wait(epfd, &result, 1, 2000) == 1);
        REQUIRE(result.events == EPOLLIN && result.data.u64 == event.data.u64);
    }
    REQUIRE(close(epfd) == 0);
    return 0;
}
static volatile sig_atomic_t interrupted;
static void poll_signal(int signo) { interrupted = signo; }
static void *interrupt_poll(void *opaque) {
    pthread_t *target = opaque;
    usleep(30000);
    return (void *)(intptr_t)pthread_kill(*target, SIGINT);
}
static int poll_interruption(int fd) {
    struct sigaction action = {.sa_handler = poll_signal}, old;
    sigemptyset(&action.sa_mask);
    REQUIRE(sigaction(SIGINT, &action, &old) == 0);
    pthread_t caller = pthread_self(), worker;
    REQUIRE(pthread_create(&worker, NULL, interrupt_poll, &caller) == 0);
    struct pollfd pfd = {.fd = fd, .events = POLLIN};
    REQUIRE(poll(&pfd, 1, 2000) == -1 && errno == EINTR);
    void *result;
    REQUIRE(pthread_join(worker, &result) == 0 && result == NULL && interrupted == SIGINT);
    REQUIRE(sigaction(SIGINT, &old, NULL) == 0);
    REQUIRE(poll_readable(fd, 20, 0) == 0);
    return 0;
}
struct flush_waiter { int fd; int ready; };
static void *wait_for_flush(void *opaque) {
    struct flush_waiter *waiter = opaque;
    if (send_command(waiter->fd, BC_ENTER_LOOPER, NULL, 0) || write(waiter->ready, "R", 1) != 1)
        return (void *)1;
    if (receive(waiter->fd, BR_NOOP, NULL)) return (void *)2;
    if (fcntl(waiter->fd, F_SETFL, O_NONBLOCK) != 0) return (void *)3;
    uint32_t command = 0;
    struct binder_write_read bwr = {.read_size = sizeof(command), .read_buffer = (uintptr_t)&command};
    if (ioctl(waiter->fd, BINDER_WRITE_READ, &bwr) != -1 || errno != EAGAIN) return (void *)4;
    return NULL;
}
static int descriptor_flush(void) {
    for (int replace = 0; replace != 2; ++replace) {
        struct peer peer;
        REQUIRE(open_peer(&peer) == 0);
        int duplicate = dup(peer.fd), ready[2];
        REQUIRE(duplicate >= 0 && pipe(ready) == 0);
        struct flush_waiter waiter = {.fd = peer.fd, .ready = ready[1]};
        pthread_t worker;
        REQUIRE(pthread_create(&worker, NULL, wait_for_flush, &waiter) == 0);
        char registered;
        REQUIRE(read(ready[0], &registered, 1) == 1 && registered == 'R');
        if (replace) {
            int other = open("/dev/null", O_RDWR | O_CLOEXEC);
            REQUIRE(other >= 0 && dup3(other, duplicate, O_CLOEXEC) == duplicate);
            REQUIRE(close(other) == 0 && close(duplicate) == 0);
        } else {
            REQUIRE(close(duplicate) == 0);
        }
        void *result = NULL;
        REQUIRE(pthread_join(worker, &result) == 0 && result == NULL);
        struct binder_version version = {0};
        REQUIRE(ioctl(peer.fd, BINDER_VERSION, &version) == 0 && version.protocol_version == 8);
        REQUIRE(close(ready[0]) == 0 && close(ready[1]) == 0);
        REQUIRE(close(peer.fd) == 0 && munmap(peer.pool, POOL_SIZE) == 0);
    }
    printf("SCARLET_BINDER_FLUSH_OK close dup3 duplicate-live\n");
    return 0;
}
static int server(int ready) {
    struct peer peer;
    REQUIRE(open_peer(&peer) == 0);
    struct flat_binder_object node = {.hdr.type = BINDER_TYPE_BINDER, .binder = 0x123400, .cookie = 0x567800};
    REQUIRE(ioctl(peer.fd, BINDER_SET_CONTEXT_MGR_EXT, &node) == 0);
    REQUIRE(send_command(peer.fd, BC_ENTER_LOOPER, NULL, 0) == 0);
    REQUIRE(poll_readable(peer.fd, 20, 0) == 0);
    REQUIRE(write(ready, "R", 1) == 1);
    close(ready);
    struct binder_transaction_data tx;
    REQUIRE(poll_readable(peer.fd, 2000, 1) == 0);
    REQUIRE(epoll_level(peer.fd) == 0);
    REQUIRE(receive(peer.fd, BR_TRANSACTION, &tx) == 0);
    REQUIRE(poll_readable(peer.fd, 0, 0) == 0);
    REQUIRE(tx.target.ptr == node.binder && tx.cookie == node.cookie);
    REQUIRE(tx.code == 42 && tx.sender_pid == getppid() && tx.sender_euid == geteuid());
    REQUIRE(tx.data_size == 5 && !memcmp((void *)(uintptr_t)tx.data.ptr.buffer, "ping", 5));
    REQUIRE(free_buffer(peer.fd, &tx) == 0);
    REQUIRE(free_buffer(peer.fd, &tx) == -1 && errno == EINVAL);
    struct binder_transaction_data reply = {.code = 43, .data_size = 5, .data.ptr.buffer = (uintptr_t)"pong"};
    usleep(30000);
    REQUIRE(send_command(peer.fd, BC_REPLY, &reply, sizeof(reply)) == 0);
    REQUIRE(receive(peer.fd, BR_TRANSACTION_COMPLETE, NULL) == 0);
    REQUIRE(poll_readable(peer.fd, 2000, 1) == 0);
    REQUIRE(receive(peer.fd, BR_TRANSACTION, &tx) == 0);
    REQUIRE(tx.code == 99);
    // Close with a live receive VMA and an unanswered call. The client must
    // receive BR_DEAD_REPLY even though the receive pages are still mapped.
    REQUIRE(close(peer.fd) == 0);
    REQUIRE(munmap(peer.pool, POOL_SIZE) == 0);
    return 0;
}
int binder_case(void) {
    REQUIRE(descriptor_flush() == 0);
    int pipefd[2];
    REQUIRE(pipe(pipefd) == 0);
    pid_t child = fork();
    REQUIRE(child >= 0);
    if (!child) { close(pipefd[0]); _exit(server(pipefd[1])); }
    close(pipefd[1]);
    char ready;
    REQUIRE(read(pipefd[0], &ready, 1) == 1 && ready == 'R');
    close(pipefd[0]);
    struct peer peer;
    REQUIRE(open_peer(&peer) == 0);
    REQUIRE(poll_interruption(peer.fd) == 0);
    REQUIRE(ioctl(peer.fd, BINDER_SET_CONTEXT_MGR, 0) == -1 && errno == EBUSY);
    // Inherited descriptors cannot act for another process; receive VMAs have
    // VM_DONTCOPY semantics, checked by reserving the same address in the child.
    pid_t inherited = fork();
    REQUIRE(inherited >= 0);
    if (!inherited) {
        struct binder_version version;
        int good = ioctl(peer.fd, BINDER_VERSION, &version) == -1 && errno == EINVAL;
        void *p = mmap(peer.pool, POOL_SIZE, PROT_READ, MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED_NOREPLACE, -1, 0);
        _exit(good && p == peer.pool ? 0 : 1);
    }
    int status;
    REQUIRE(waitpid(inherited, &status, 0) == inherited && status == 0);
    struct binder_transaction_data tx = {.code = 42, .data_size = 5, .data.ptr.buffer = (uintptr_t)"ping"};
    binder_uintptr_t bad = (uintptr_t)peer.pool + 15 * 65536;
    REQUIRE(send_command(peer.fd, BC_FREE_BUFFER, &bad, sizeof(bad)) == -1 && errno == EINVAL);
    tx.offsets_size = 8;
    REQUIRE(send_command(peer.fd, BC_TRANSACTION, &tx, sizeof(tx)) == -1 && errno == EFAULT);
    tx.offsets_size = 0;
    REQUIRE(send_command(peer.fd, BC_TRANSACTION, &tx, sizeof(tx)) == 0);
    REQUIRE(receive(peer.fd, BR_TRANSACTION_COMPLETE, NULL) == 0);
    REQUIRE(poll_readable(peer.fd, 2000, 1) == 0);
    REQUIRE(epoll_level(peer.fd) == 0);
    struct binder_transaction_data reply;
    REQUIRE(receive(peer.fd, BR_REPLY, &reply) == 0);
    REQUIRE(reply.code == 43 && reply.data_size == 5 && !memcmp((void *)(uintptr_t)reply.data.ptr.buffer, "pong", 5));
    REQUIRE(free_buffer(peer.fd, &reply) == 0);
    REQUIRE(poll_readable(peer.fd, 20, 0) == 0);
    tx.code = 99;
    REQUIRE(send_command(peer.fd, BC_TRANSACTION, &tx, sizeof(tx)) == 0);
    REQUIRE(receive(peer.fd, BR_TRANSACTION_COMPLETE, NULL) == 0);
    REQUIRE(poll_readable(peer.fd, 2000, 1) == 0);
    REQUIRE(receive(peer.fd, BR_DEAD_REPLY, NULL) == 0);
    REQUIRE(waitpid(child, &status, 0) == child && status == 0);
    REQUIRE(munmap(peer.pool, POOL_SIZE) == 0 && close(peer.fd) == 0);
    printf("SCARLET_BINDER_RPC_OK request reply buffer-free peer-death fork-isolation poll-timeout poll-interrupt epoll-level\n");
    extern int binder_objects_case(void);
    return binder_objects_case();
}
