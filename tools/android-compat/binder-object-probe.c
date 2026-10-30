/*
 * Bounded synchronous ARM64 Binder object protocol, using stock NDK headers.
 * Run by the existing static/dynamic `binder` case after its raw RPC checks.
 *
 * Three independently opened Binder connections, in three actual processes:
 *   manager: context node plus a distinct service and a weak-only object;
 *   client: obtains service/weak/forwarder handles in the manager's reply;
 *   forwarder: registers its own endpoint, receives capabilities from client,
 *              replies, then independently calls/roundtrips them to manager.
 * Every received handle is retained before BC_FREE_BUFFER. No nested calls,
 * oneway calls, weak promotion, death subscription, or FD transfer is needed.
 *
 * Receive offsets must follow ALIGN(data_size, 8) in the read-only pool.
 * Objects deliberately start at offset 4 (legal, not eight-byte aligned).
 * Both request and reply paths exercise 64 24-byte flat objects; 65 is E2BIG.
 * Malformed layouts/unknown handles require EINVAL, inaccessible offsets
 * EFAULT, FD delivery to a non-accepting target EPERM, and weak-only
 * acquire/call EOPNOTSUPP. A successful
 * write, generic failure reply, or ignored unsupported command is no pass.
 * Rejected prefixes exercise rollback of strong, weak and new local objects.
 * Final RELEASE/DECREFS are checked with all three connections still open,
 * including a separate strong-zero/weak-live checkpoint. The service is then
 * exported again; after its owner exits, a nonzero-handle call must return
 * BR_DEAD_REPLY. Parent/children use deadlines and failure cleanup/reaping.
 */
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <poll.h>
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/mman.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>
#include <linux/android/binder.h>

_Static_assert(sizeof(void *) == 8 && sizeof(binder_size_t) == 8, "64-bit Binder only");
_Static_assert(sizeof(struct flat_binder_object) == 24, "stock flat object layout");
_Static_assert(sizeof(struct binder_transaction_data) == 64, "stock transaction layout");

#define POOL_SIZE (1024 * 1024 - 8192)
#define MAX_OBJECTS 64
#define TIMEOUT_MS 5000
#define MAX_PUMPS 512
#define PACKET_MAGIC UINT32_C(0x5ca17e64)
#define TOKEN UINT64_C(0x7193ac48fed052b6)
#define UNKNOWN_HANDLE UINT32_C(0x7ffffffe)

/* Opaque identities: deliberately nonzero above bit 31; never dereferenced. */
enum node_id { CONTEXT, SERVICE, WEAK_ONLY, ENDPOINT, ROLLBACK, NODE_COUNT };
static const struct binder_ptr_cookie identities[NODE_COUNT] = {
    {UINT64_C(0x123400001000), UINT64_C(0x567800001800)},
    {UINT64_C(0x123400002000), UINT64_C(0x567800002800)},
    {UINT64_C(0x123400003000), UINT64_C(0x567800003800)},
    {UINT64_C(0x123400004000), UINT64_C(0x567800004800)},
    {UINT64_C(0x123400005000), UINT64_C(0x567800005800)},
};
enum code {
    REGISTER = 0x601, EXPORT, PING, ROUNDTRIP, MANY_REQUEST, MANY_REPLY,
    FORWARD, CHECK_LIVE, CHECK_WEAK, CHECK_ZERO, REEXPORT, STOP_OWNER, BAD,
};
static const char *role = "client";
static const char *phase = "setup";
static int failure(int line, const char *condition) {
    int saved_errno = errno;
    dprintf(STDERR_FILENO, "SCARLET_BIONIC_FAIL binder-objects: %s/%s line %d: %s errno=%d\n",
            role, phase, line, condition, saved_errno);
    return -1;
}
#define CHECK(x) do { if (!(x)) return failure(__LINE__, #x); } while (0)

struct node_refs {
    unsigned strong, weak, increfs, acquires, releases, decrefs;
};
struct event {
    uint32_t command;
    struct binder_transaction_data tx;
};
struct object_peer {
    int fd;
    void *pool;
    unsigned owned;
    struct node_refs refs[NODE_COUNT];
    struct event pending[8];
    size_t pending_count;
};
struct packet {
    unsigned char data[4 + (MAX_OBJECTS + 1) * sizeof(struct flat_binder_object) + 3];
    binder_size_t offsets[MAX_OBJECTS + 1];
    size_t count, size;
};
struct handles { uint32_t service, weak, endpoint; };

static int64_t now_ms(void) {
    struct timespec ts;
    if (clock_gettime(CLOCK_MONOTONIC, &ts)) return -1;
    return (int64_t)ts.tv_sec * 1000 + ts.tv_nsec / 1000000;
}
static int remaining(int64_t deadline) {
    int64_t now = now_ms();
    if (now < 0) return -1;
    if (now >= deadline) { errno = ETIMEDOUT; return 0; }
    return (int)(deadline - now);
}
static int poll_until(struct pollfd *fds, nfds_t count, int timeout) {
    int64_t deadline = now_ms() + timeout;
    for (;;) {
        int result = poll(fds, count, timeout);
        if (result >= 0 || errno != EINTR) return result;
        timeout = remaining(deadline);
        if (timeout <= 0) return timeout;
    }
}
static int command(int fd, uint32_t cmd, const void *payload, size_t size) {
    unsigned char bytes[sizeof(cmd) + sizeof(struct binder_transaction_data_sg)];
    CHECK(size <= sizeof(bytes) - sizeof(cmd));
    memcpy(bytes, &cmd, sizeof(cmd));
    if (size) memcpy(bytes + sizeof(cmd), payload, size);
    struct binder_write_read bwr = {
        .write_size = sizeof(cmd) + size, .write_buffer = (uintptr_t)bytes,
    };
    if (ioctl(fd, BINDER_WRITE_READ, &bwr)) return -1;
    CHECK(bwr.write_consumed == sizeof(cmd) + size);
    return 0;
}
static int peer_open_path(struct object_peer *p, unsigned owned,const char *path) {
    p->owned = owned;
    p->fd = open(path, O_RDWR | O_CLOEXEC | O_NONBLOCK);
    CHECK(p->fd >= 0);
    struct binder_version version = {0};
    CHECK(ioctl(p->fd, BINDER_VERSION, &version) == 0 && version.protocol_version == 8);
    p->pool = mmap(NULL, POOL_SIZE, PROT_READ, MAP_PRIVATE | MAP_NORESERVE, p->fd, 0);
    CHECK(p->pool != MAP_FAILED);
    CHECK(command(p->fd, BC_ENTER_LOOPER, NULL, 0) == 0);
    return 0;
}
static int peer_open(struct object_peer *p,unsigned owned) {return peer_open_path(p,owned,"/dev/binder");}
static int peer_close(struct object_peer *p) {
    int result = 0;
    if (p->pool != MAP_FAILED) {
        if (munmap(p->pool, POOL_SIZE)) result = -1;
        p->pool = MAP_FAILED;
    }
    if (p->fd >= 0) {
        if (close(p->fd)) result = -1;
        p->fd = -1;
    }
    return result;
}
static int ref_record(struct object_peer *p, uint32_t cmd, const unsigned char *data) {
    struct binder_ptr_cookie ref;
    memcpy(&ref, data, sizeof(ref));
    unsigned node;
    for (node = 0; node < NODE_COUNT; ++node)
        if ((p->owned & (1u << node)) && ref.ptr == identities[node].ptr &&
            ref.cookie == identities[node].cookie) break;
    CHECK(node < NODE_COUNT);
    struct node_refs *r = &p->refs[node];
    switch (cmd) {
    case BR_INCREFS:
        CHECK(++r->increfs <= 4096);
        ++r->weak;
        CHECK(command(p->fd, BC_INCREFS_DONE, &ref, sizeof(ref)) == 0);
        break;
    case BR_ACQUIRE:
        CHECK(++r->acquires <= 4096);
        ++r->strong;
        CHECK(command(p->fd, BC_ACQUIRE_DONE, &ref, sizeof(ref)) == 0);
        break;
    case BR_RELEASE:
        CHECK(r->strong > 0);
        --r->strong;
        ++r->releases;
        break;
    case BR_DECREFS:
        CHECK(r->weak > 0);
        --r->weak;
        ++r->decrefs;
        break;
    default: CHECK(0);
    }
    return 0;
}
/* Parse the ENTIRE read, including ref records following a reply. Acknowledge
 * INCREFS/ACQUIRE immediately; preserve RPC records even when coalesced with
 * NOOP, ref changes and transaction-complete records. No read is one-record. */
static int pump(struct object_peer *p, int timeout) {
    struct pollfd fd = {.fd = p->fd, .events = POLLIN};
    int ready = poll_until(&fd, 1, timeout);
    CHECK(ready >= 0);
    if (!ready) return 0;
    CHECK((fd.revents & POLLIN) && !(fd.revents & (POLLERR | POLLNVAL)));
    unsigned char bytes[4096];
    struct binder_write_read bwr = {.read_size = sizeof(bytes), .read_buffer = (uintptr_t)bytes};
    int result = ioctl(p->fd, BINDER_WRITE_READ, &bwr);
    if (result < 0 && (errno == EAGAIN || errno == EINTR)) return 0;
    CHECK(result == 0 && bwr.read_consumed >= 4 && bwr.read_consumed <= sizeof(bytes));
    for (size_t pos = 0; pos < bwr.read_consumed;) {
        uint32_t cmd;
        CHECK(bwr.read_consumed - pos >= sizeof(cmd));
        memcpy(&cmd, bytes + pos, sizeof(cmd));
        pos += sizeof(cmd);
        size_t size = _IOC_SIZE(cmd);
        CHECK(size <= bwr.read_consumed - pos);
        switch (cmd) {
        case BR_NOOP:
        case BR_TRANSACTION_COMPLETE:
            break;
        case BR_INCREFS:
        case BR_ACQUIRE:
        case BR_RELEASE:
        case BR_DECREFS:
            CHECK(ref_record(p, cmd, bytes + pos) == 0);
            break;
        case BR_TRANSACTION:
        case BR_REPLY:
        case BR_DEAD_REPLY: {
            CHECK(p->pending_count < sizeof(p->pending) / sizeof(p->pending[0]));
            struct event *event = &p->pending[p->pending_count++];
            memset(event, 0, sizeof(*event));
            event->command = cmd;
            if (cmd != BR_DEAD_REPLY) memcpy(&event->tx, bytes + pos, sizeof(event->tx));
            break;
        }
        default:
            dprintf(STDERR_FILENO, "binder-objects: unexpected BR command %#x\n", cmd);
            CHECK(0); /* Includes failed/error replies: never pretend success. */
        }
        pos += size;
    }
    return 1;
}
static int next_event(struct object_peer *p, uint32_t expected, struct binder_transaction_data *tx) {
    int64_t deadline = now_ms() + TIMEOUT_MS;
    for (unsigned attempt = 0; !p->pending_count; ++attempt) {
        int timeout = remaining(deadline);
        CHECK(attempt < MAX_PUMPS && timeout > 0);
        CHECK(pump(p, timeout) >= 0);
    }
    struct event event = p->pending[0];
    --p->pending_count;
    memmove(p->pending, p->pending + 1, p->pending_count * sizeof(p->pending[0]));
    if (event.command != expected)
        dprintf(STDERR_FILENO, "binder-objects: expected BR %#x, received %#x (code %#x)\n",
                expected, event.command, event.tx.code);
    CHECK(event.command == expected);
    if (tx) *tx = event.tx;
    return 0;
}
static int quiet(struct object_peer *p) {
    int64_t deadline = now_ms() + TIMEOUT_MS;
    for (unsigned i = 0; i < MAX_PUMPS; ++i) {
        CHECK(p->pending_count == 0 && remaining(deadline) > 0);
        int result = pump(p, 20);
        CHECK(result >= 0 && p->pending_count == 0);
        if (!result) return 0;
    }
    CHECK(0);
}
static int wait_refs(struct object_peer *p, enum node_id node, int strong, int weak) {
    int64_t deadline = now_ms() + TIMEOUT_MS;
    for (unsigned i = 0; i < MAX_PUMPS; ++i) {
        /* Drain already available work even when the initial counters match. */
        CHECK(pump(p, 0) >= 0 && p->pending_count == 0);
        struct node_refs *r = &p->refs[node];
        if (!!r->strong == strong && !!r->weak == weak) return 0;
        int timeout = remaining(deadline);
        CHECK(timeout > 0 && pump(p, timeout) >= 0 && p->pending_count == 0);
    }
    CHECK(0);
}
static int balanced(const struct object_peer *p, enum node_id node, int require_refs) {
    const struct node_refs *r = &p->refs[node];
    CHECK(r->strong == 0 && r->weak == 0);
    CHECK(r->increfs == r->decrefs && r->acquires == r->releases);
    if (require_refs) CHECK(r->increfs > 0 && r->acquires > 0);
    return 0;
}
static int in_pool(const struct object_peer *p, binder_uintptr_t ptr, binder_size_t size) {
    binder_uintptr_t start = (uintptr_t)p->pool;
    return ptr >= start && size <= POOL_SIZE && ptr - start <= POOL_SIZE - size;
}
static int free_received(struct object_peer *p, const struct binder_transaction_data *tx) {
    binder_uintptr_t buffer = tx->data.ptr.buffer;
    CHECK(command(p->fd, BC_FREE_BUFFER, &buffer, sizeof(buffer)) == 0);
    if (tx->flags & TF_CLEAR_BUF) {
        CHECK(in_pool(p, buffer, tx->data_size));
        const volatile unsigned char *bytes = (const volatile unsigned char *)(uintptr_t)buffer;
        for (binder_size_t i = 0; i < tx->data_size; ++i) CHECK(bytes[i] == 0);
    }
    return 0;
}
static int retain(struct object_peer *p, uint32_t handle, int strong) {
    CHECK(handle != 0);
    CHECK(command(p->fd, BC_INCREFS, &handle, sizeof(handle)) == 0);
    if (strong) CHECK(command(p->fd, BC_ACQUIRE, &handle, sizeof(handle)) == 0);
    return 0;
}
static int release(struct object_peer *p, uint32_t handle, int strong) {
    if (strong) CHECK(command(p->fd, BC_RELEASE, &handle, sizeof(handle)) == 0);
    CHECK(command(p->fd, BC_DECREFS, &handle, sizeof(handle)) == 0);
    return 0;
}
static struct flat_binder_object local_object(enum node_id node, int strong) {
    return (struct flat_binder_object){
        .hdr.type = strong ? BINDER_TYPE_BINDER : BINDER_TYPE_WEAK_BINDER,
        .binder = identities[node].ptr, .cookie = identities[node].cookie,
    };
}
static struct flat_binder_object remote_object(uint32_t handle, int strong) {
    return (struct flat_binder_object){
        .hdr.type = strong ? BINDER_TYPE_HANDLE : BINDER_TYPE_WEAK_HANDLE, .handle = handle,
    };
}
static void packet_init(struct packet *p, size_t count) {
    memset(p, 0, sizeof(*p));
    p->count = count;
    p->size = 4 + count * sizeof(struct flat_binder_object) + 3;
    uint32_t magic = PACKET_MAGIC;
    memcpy(p->data, &magic, sizeof(magic));
    memcpy(p->data + p->size - 3, "end", 3);
    for (size_t i = 0; i < count; ++i) p->offsets[i] = 4 + i * sizeof(struct flat_binder_object);
}
static void packet_object(struct packet *p, size_t index, struct flat_binder_object object) {
    memcpy(p->data + 4 + index * sizeof(object), &object, sizeof(object));
}
static struct binder_transaction_data packet_tx(const struct packet *p, uint32_t handle, uint32_t code) {
    return (struct binder_transaction_data){
        .target.handle = handle, .code = code, .data_size = p->size,
        .offsets_size = p->count * sizeof(binder_size_t),
        .data.ptr = {.buffer = (uintptr_t)p->data, .offsets = (uintptr_t)p->offsets},
    };
}
static int send_packet(struct object_peer *p, uint32_t cmd, uint32_t handle, uint32_t code,
                       const struct packet *packet) {
    struct binder_transaction_data tx = packet_tx(packet, handle, code);
    CHECK(command(p->fd, cmd, &tx, sizeof(tx)) == 0);
    return 0;
}
static int read_packet(struct object_peer *p, const struct binder_transaction_data *tx,
                       size_t count, struct flat_binder_object *objects) {
    CHECK(count <= MAX_OBJECTS);
    CHECK(!(tx->flags & (TF_ONE_WAY | TF_STATUS_CODE)));
    CHECK(tx->data_size == 4 + count * sizeof(*objects) + 3);
    CHECK(tx->offsets_size == count * sizeof(binder_size_t));
    CHECK(in_pool(p, tx->data.ptr.buffer, tx->data_size));
    CHECK(tx->data.ptr.offsets == tx->data.ptr.buffer + ((tx->data_size + 7) & ~(binder_size_t)7));
    CHECK(!(tx->data.ptr.offsets & 7) && in_pool(p, tx->data.ptr.offsets, tx->offsets_size));
    const unsigned char *data = (const void *)(uintptr_t)tx->data.ptr.buffer;
    const unsigned char *offsets = (const void *)(uintptr_t)tx->data.ptr.offsets;
    uint32_t magic;
    memcpy(&magic, data, sizeof(magic));
    CHECK(magic == PACKET_MAGIC && !memcmp(data + tx->data_size - 3, "end", 3));
    for (size_t i = 0; i < count; ++i) {
        binder_size_t offset;
        memcpy(&offset, offsets + i * sizeof(offset), sizeof(offset));
        CHECK(offset == 4 + i * sizeof(*objects));
        memcpy(objects + i, data + offset, sizeof(*objects));
    }
    return 0;
}
static int is_local(const struct flat_binder_object *object, enum node_id node, int strong) {
    CHECK(object->hdr.type == (strong ? BINDER_TYPE_BINDER : BINDER_TYPE_WEAK_BINDER));
    CHECK(object->binder == identities[node].ptr && object->cookie == identities[node].cookie);
    return 0;
}
static int is_remote(const struct flat_binder_object *object, int strong) {
    CHECK(object->hdr.type == (strong ? BINDER_TYPE_HANDLE : BINDER_TYPE_WEAK_HANDLE));
    /* The unused high union word and owner cookie must not leak to receivers. */
    CHECK(object->handle != 0 && object->binder == object->handle && object->cookie == 0);
    return 0;
}
static int send_scalar(struct object_peer *p, uint32_t cmd, uint32_t handle, uint32_t code) {
    uint64_t data = TOKEN ^ code;
    struct binder_transaction_data tx = {
        .target.handle = handle, .code = code, .flags = TF_CLEAR_BUF, .data_size = sizeof(data), .data.ptr.buffer = (uintptr_t)&data,
    };
    /* target.handle occupies only the low word of this union. Real C
     * callers need not initialize its unused upper word. */
    tx.target.ptr |= UINT64_C(0xa5a5a5a5) << 32;
    CHECK(command(p->fd, cmd, &tx, sizeof(tx)) == 0);
    return 0;
}
static int read_scalar(struct object_peer *p, const struct binder_transaction_data *tx, uint32_t code) {
    CHECK(tx->code == code && !(tx->flags & (TF_ONE_WAY | TF_STATUS_CODE)) && (tx->flags & TF_CLEAR_BUF));
    CHECK(tx->data_size == sizeof(uint64_t) && tx->offsets_size == 0);
    CHECK(in_pool(p, tx->data.ptr.buffer, tx->data_size));
    uint64_t data;
    memcpy(&data, (const void *)(uintptr_t)tx->data.ptr.buffer, sizeof(data));
    CHECK(data == (TOKEN ^ code));
    CHECK(free_received(p, tx) == 0);
    return 0;
}
static int scalar_reply(struct object_peer *p, uint32_t code) {
    struct binder_transaction_data reply;
    CHECK(next_event(p, BR_REPLY, &reply) == 0 && read_scalar(p, &reply, code) == 0);
    return 0;
}
static int call_scalar(struct object_peer *p, uint32_t handle, uint32_t code) {
    CHECK(send_scalar(p, BC_TRANSACTION, handle, code) == 0 && scalar_reply(p, code) == 0);
    return 0;
}
static int target_is(const struct binder_transaction_data *tx, enum node_id node, pid_t sender) {
    CHECK(tx->target.ptr == identities[node].ptr && tx->cookie == identities[node].cookie);
    CHECK(tx->sender_pid == sender && tx->sender_euid == geteuid());
    return 0;
}
static int pipe_byte(int fd, char expected) {
    struct pollfd pfd = {.fd = fd, .events = POLLIN};
    CHECK(poll_until(&pfd, 1, TIMEOUT_MS) == 1 && (pfd.revents & POLLIN));
    char byte;
    CHECK(read(fd, &byte, 1) == 1 && byte == expected);
    return 0;
}

static uint32_t normal_flags(size_t index) {
    return (index & 1 ? 19u : 0u) | (index & 2 ? FLAT_BINDER_FLAG_ACCEPTS_FDS : 0u);
}
static int roundtrip(struct object_peer *p, const struct handles *h) {
    struct packet packet;
    packet_init(&packet, 3);
    packet_object(&packet, 0, remote_object(h->service, 1));
    packet_object(&packet, 1, remote_object(h->service, 0));
    packet_object(&packet, 2, remote_object(h->weak, 0));
    CHECK(send_packet(p, BC_TRANSACTION, h->service, ROUNDTRIP, &packet) == 0);
    CHECK(scalar_reply(p, ROUNDTRIP) == 0);
    return 0;
}
static int manager_refs(struct object_peer *p, uint32_t code) {
    int strong = code == CHECK_LIVE;
    int weak = code != CHECK_ZERO;
    CHECK(wait_refs(p, SERVICE, strong, weak) == 0);
    CHECK(wait_refs(p, WEAK_ONLY, 0, weak) == 0);
    CHECK(p->refs[SERVICE].acquires > 0 && p->refs[SERVICE].increfs > 0);
    CHECK(p->refs[WEAK_ONLY].acquires == 0 && p->refs[WEAK_ONLY].increfs > 0);
    if (code == CHECK_WEAK) {
        CHECK(p->refs[SERVICE].releases > 0);
        printf("SCARLET_BINDER_OBJECTS_REFS strong-zero weak-live\n");
    }
    if (code == CHECK_ZERO) {
        CHECK(quiet(p) == 0 && balanced(p, SERVICE, 1) == 0 && balanced(p, WEAK_ONLY, 0) == 0);
        printf("SCARLET_BINDER_OBJECTS_REFS balanced-before-close\n");
    }
    return 0;
}
static int manager_main(int ready, pid_t client_pid) {
    role = "manager";
    struct object_peer p = {.fd = -1, .pool = MAP_FAILED};
    CHECK(peer_open(&p, (1u << CONTEXT) | (1u << SERVICE) | (1u << WEAK_ONLY)) == 0);
    struct flat_binder_object context = local_object(CONTEXT, 1);
    CHECK(ioctl(p.fd, BINDER_SET_CONTEXT_MGR_EXT, &context) == 0);
    CHECK(write(ready, "R", 1) == 1 && close(ready) == 0);
    uint32_t endpoint = 0;
    pid_t forwarder_pid = -1;
    for (unsigned calls = 0; calls < 32; ++calls) {
        struct binder_transaction_data tx;
        struct flat_binder_object objects[MAX_OBJECTS];
        struct packet packet;
        phase = "receive";
        CHECK(next_event(&p, BR_TRANSACTION, &tx) == 0);
        int service_call = tx.code == PING || tx.code == ROUNDTRIP ||
                           tx.code == MANY_REQUEST || tx.code == MANY_REPLY;
        pid_t sender = client_pid;
        if (tx.code == REGISTER) {
            CHECK(forwarder_pid == -1 && tx.sender_pid > 0 && tx.sender_pid != client_pid);
            sender = tx.sender_pid;
        } else if (tx.code == PING || tx.code == ROUNDTRIP) {
            CHECK(tx.sender_pid == client_pid || tx.sender_pid == forwarder_pid);
            sender = tx.sender_pid;
        }
        CHECK(target_is(&tx, service_call ? SERVICE : CONTEXT, sender) == 0);
        switch (tx.code) {
        case REGISTER:
            phase = "register-forwarder";
            CHECK(read_packet(&p, &tx, 1, objects) == 0 && is_remote(objects, 1) == 0);
            endpoint = objects[0].handle;
            forwarder_pid = tx.sender_pid;
            CHECK(retain(&p, endpoint, 1) == 0 && free_received(&p, &tx) == 0);
            CHECK(send_scalar(&p, BC_REPLY, 0, tx.code) == 0);
            break;
        case EXPORT:
        case REEXPORT:
            phase = "export-service";
            CHECK(endpoint != 0 && read_scalar(&p, &tx, tx.code) == 0);
            packet_init(&packet, tx.code == EXPORT ? 4 : 2);
            packet_object(&packet, 0, local_object(SERVICE, 1));
            packet_object(&packet, 1, local_object(SERVICE, 0));
            if (tx.code == EXPORT) {
                packet_object(&packet, 2, local_object(WEAK_ONLY, 0));
                packet_object(&packet, 3, remote_object(endpoint, 1));
            }
            CHECK(send_packet(&p, BC_REPLY, 0, tx.code, &packet) == 0);
            break;
        case PING:
            phase = "service-call";
            CHECK(read_scalar(&p, &tx, tx.code) == 0);
            CHECK(send_scalar(&p, BC_REPLY, 0, tx.code) == 0);
            break;
        case ROUNDTRIP:
            phase = "owner-roundtrip";
            CHECK(read_packet(&p, &tx, 3, objects) == 0);
            CHECK(is_local(objects, SERVICE, 1) == 0 && is_local(objects + 1, SERVICE, 0) == 0);
            CHECK(is_local(objects + 2, WEAK_ONLY, 0) == 0);
            CHECK(free_received(&p, &tx) == 0 && send_scalar(&p, BC_REPLY, 0, tx.code) == 0);
            break;
        case MANY_REQUEST:
            phase = "64-request-objects";
            CHECK(read_packet(&p, &tx, MAX_OBJECTS, objects) == 0);
            for (size_t i = 0; i < MAX_OBJECTS; ++i) {
                CHECK(is_local(objects + i, SERVICE, !(i & 1)) == 0);
            }
            CHECK(free_received(&p, &tx) == 0 && send_scalar(&p, BC_REPLY, 0, tx.code) == 0);
            break;
        case MANY_REPLY:
            phase = "64-reply-objects";
            CHECK(read_scalar(&p, &tx, tx.code) == 0);
            packet_init(&packet, MAX_OBJECTS);
            for (size_t i = 0; i < MAX_OBJECTS; ++i) {
                struct flat_binder_object object = local_object(SERVICE, !(i & 1));
                packet_object(&packet, i, object);
            }
            CHECK(send_packet(&p, BC_REPLY, 0, tx.code, &packet) == 0);
            break;
        case CHECK_LIVE:
        case CHECK_WEAK:
        case CHECK_ZERO:
            phase = "ref-lifetime";
            CHECK(read_scalar(&p, &tx, tx.code) == 0 && manager_refs(&p, tx.code) == 0);
            CHECK(send_scalar(&p, BC_REPLY, 0, tx.code) == 0);
            break;
        case STOP_OWNER:
            phase = "owner-exit";
            CHECK(read_scalar(&p, &tx, tx.code) == 0);
            CHECK(wait_refs(&p, SERVICE, 1, 1) == 0);
            CHECK(release(&p, endpoint, 1) == 0);
            CHECK(send_scalar(&p, BC_REPLY, 0, tx.code) == 0);
            CHECK(peer_close(&p) == 0);
            return 0;
        default:
            dprintf(STDERR_FILENO, "binder-objects: manager unexpected transaction %#x\n", tx.code);
            CHECK(0);
        }
    }
    CHECK(0);
}

static int read_capabilities(struct object_peer *p, const struct binder_transaction_data *tx,
                             struct handles *h, int with_endpoint) {
    struct flat_binder_object objects[4];
    CHECK(read_packet(p, tx, with_endpoint ? 4 : 3, objects) == 0);
    CHECK(is_remote(objects, 1) == 0 && is_remote(objects + 1, 0) == 0);
    CHECK(objects[0].handle == objects[1].handle && is_remote(objects + 2, 0) == 0);
    h->service = objects[0].handle;
    h->weak = objects[2].handle;
    CHECK(h->weak != h->service);
    CHECK(retain(p, h->service, 1) == 0 && retain(p, h->weak, 0) == 0);
    if (with_endpoint) {
        CHECK(is_remote(objects + 3, 1) == 0);
        h->endpoint = objects[3].handle;
        CHECK(h->endpoint != h->service && h->endpoint != h->weak);
        CHECK(retain(p, h->endpoint, 1) == 0);
    }
    CHECK(free_received(p, tx) == 0);
    return 0;
}
static int forwarder_main(int control, int report, pid_t client_pid) {
    role = "forwarder";
    struct object_peer p = {.fd = -1, .pool = MAP_FAILED};
    CHECK(peer_open(&p, 1u << ENDPOINT) == 0);
    struct packet packet;
    packet_init(&packet, 1);
    packet_object(&packet, 0, local_object(ENDPOINT, 1));
    CHECK(send_packet(&p, BC_TRANSACTION, 0, REGISTER, &packet) == 0 && scalar_reply(&p, REGISTER) == 0);
    CHECK(write(report, "R", 1) == 1);
    phase = "receive-capability";
    struct binder_transaction_data tx;
    struct handles h = {0};
    CHECK(next_event(&p, BR_TRANSACTION, &tx) == 0);
    /* Any rejected packet delivered here is an error, including a packet that
     * spuriously got delivered even though its caller saw an ioctl failure. */
    CHECK(tx.code == FORWARD && target_is(&tx, ENDPOINT, client_pid) == 0);
    CHECK(read_capabilities(&p, &tx, &h, 0) == 0);
    CHECK(send_scalar(&p, BC_REPLY, 0, FORWARD) == 0);
    phase = "independent-service-call";
    CHECK(call_scalar(&p, h.service, PING) == 0 && roundtrip(&p, &h) == 0);
    CHECK(release(&p, h.service, 1) == 0 && release(&p, h.weak, 0) == 0);
    CHECK(write(report, "D", 1) == 1);
    /* Remain open while client checks rollback/final service refs. Continue
     * acknowledging the endpoint's refs while waiting for the exit message. */
    phase = "endpoint-lifetime";
    int64_t deadline = now_ms() + TIMEOUT_MS;
    for (unsigned i = 0; i < MAX_PUMPS; ++i) {
        CHECK(p.pending_count == 0);
        int timeout = remaining(deadline);
        struct pollfd fds[2] = {{.fd = p.fd, .events = POLLIN}, {.fd = control, .events = POLLIN}};
        CHECK(timeout > 0 && poll_until(fds, 2, timeout) > 0);
        if (fds[0].revents) CHECK(pump(&p, 0) >= 0 && p.pending_count == 0);
        if (fds[1].revents) {
            CHECK(pipe_byte(control, 'X') == 0);
            CHECK(wait_refs(&p, ENDPOINT, 0, 0) == 0 && quiet(&p) == 0);
            CHECK(balanced(&p, ENDPOINT, 1) == 0 && peer_close(&p) == 0);
            CHECK(close(control) == 0 && close(report) == 0);
            return 0;
        }
    }
    CHECK(0);
}

static int rejected(struct object_peer *p, const char *name, uint32_t cmd,
                    const void *data, size_t size, int expected_errno) {
    phase = name;
    errno = 0;
    int result = command(p->fd, cmd, data, size);
    int error = errno;
    /* The flag validation contract permits either invalid or unsupported.
     * Other negative cases deliberately check their specified exact errno. */
    if (result != -1 || (expected_errno ? error != expected_errno : error != EINVAL && error != EOPNOTSUPP))
        dprintf(STDERR_FILENO, "binder-objects: rejection %s result=%d errno=%d expected=%d\n",
                name, result, error, expected_errno);
    CHECK(result == -1);
    CHECK(expected_errno ? error == expected_errno : error == EINVAL || error == EOPNOTSUPP);
    CHECK(quiet(p) == 0);
    return 0;
}
static int rejected_tx(struct object_peer *p, const char *name,
                       const struct binder_transaction_data *tx, int expected_errno) {
    return rejected(p, name, BC_TRANSACTION, tx, sizeof(*tx), expected_errno);
}
static int negative_cases(struct object_peer *p, const struct handles *h) {
    struct packet packet;
    packet_init(&packet, 2);
    packet_object(&packet, 0, remote_object(h->service, 1));
    packet_object(&packet, 1, remote_object(h->service, 1));
    struct binder_transaction_data base = packet_tx(&packet, h->endpoint, BAD), tx;
    tx = base;
    tx.offsets_size--;
    CHECK(rejected_tx(p, "offset-size", &tx, EINVAL) == 0);
    const binder_size_t bad_offsets[] = {5, packet.size, packet.size - 16, UINT64_MAX - 7};
    const char *names[] = {"unaligned-offset", "past-data", "truncated-object", "overflow-offset"};
    for (size_t i = 0; i < sizeof(bad_offsets) / sizeof(bad_offsets[0]); ++i) {
        packet.offsets[1] = bad_offsets[i]; /* A valid strong prefix must roll back. */
        CHECK(rejected_tx(p, names[i], &base, EINVAL) == 0);
    }
    packet.offsets[1] = 4;
    CHECK(rejected_tx(p, "duplicate-offset", &base, EINVAL) == 0);
    packet.offsets[0] = 28;
    CHECK(rejected_tx(p, "decreasing-offset", &base, EINVAL) == 0);
    packet.offsets[0] = 4;
    packet.offsets[1] = 8;
    CHECK(rejected_tx(p, "overlapping-offset", &base, EINVAL) == 0);
    packet.offsets[1] = 28;
    tx = base;
    tx.data.ptr.offsets = 0;
    CHECK(rejected_tx(p, "null-offset-pointer", &tx, EFAULT) == 0);
    tx.data.ptr.offsets = 1;
    CHECK(rejected_tx(p, "inaccessible-offset-pointer", &tx, EFAULT) == 0);

    packet_object(&packet, 1, remote_object(UNKNOWN_HANDLE, 1));
    CHECK(rejected_tx(p, "unknown-strong-object", &base, EINVAL) == 0);
    packet_object(&packet, 0, remote_object(h->weak, 0));
    packet_object(&packet, 1, remote_object(UNKNOWN_HANDLE, 0));
    CHECK(rejected_tx(p, "unknown-weak-object", &base, EINVAL) == 0);
    struct flat_binder_object unsupported = {.hdr.type = UINT32_C(0xdeadbeef)};
    packet_object(&packet, 1, unsupported);
    CHECK(rejected_tx(p, "unknown-object-type", &base, EOPNOTSUPP) == 0);
    /* Use a valid object type with an invalid handle, so validation reaches
     * translation of the preceding new local node before the late failure. */
    packet_object(&packet, 1, remote_object(UNKNOWN_HANDLE, 1));
    packet_object(&packet, 0, local_object(ROLLBACK, 1));
    CHECK(rejected_tx(p, "rollback-new-strong-node", &base, EINVAL) == 0);
    packet_object(&packet, 1, remote_object(UNKNOWN_HANDLE, 0));
    packet_object(&packet, 0, local_object(ROLLBACK, 0));
    CHECK(rejected_tx(p, "rollback-new-weak-node", &base, EINVAL) == 0);
    packet_object(&packet, 0, remote_object(h->service, 1));
    unsupported.hdr.type = BINDER_TYPE_FD;
    unsupported.handle = STDOUT_FILENO;
    packet_object(&packet, 1, unsupported);
    CHECK(rejected_tx(p, "target-refuses-fd-object", &base, EPERM) == 0);

    const uint32_t bad_flags[] = {1, 1u << FLAT_BINDER_FLAG_SCHED_POLICY_SHIFT,
                                 FLAT_BINDER_FLAG_TXN_SECURITY_CTX,
                                 FLAT_BINDER_FLAG_INHERIT_RT | (1u << FLAT_BINDER_FLAG_SCHED_POLICY_SHIFT)};
    for (size_t i = 0; i < sizeof(bad_flags) / sizeof(bad_flags[0]); ++i) {
        struct flat_binder_object object = local_object(ROLLBACK, 1);
        object.flags = bad_flags[i];
        packet_object(&packet, 1, object);
        CHECK(rejected_tx(p, "unsupported-flat-flags", &base, 0) == 0);
    }

    packet_init(&packet, MAX_OBJECTS + 1);
    for (size_t i = 0; i < packet.count; ++i)
        packet_object(&packet, i, remote_object(h->service, !(i & 1)));
    tx = packet_tx(&packet, h->endpoint, BAD);
    CHECK(rejected_tx(p, "65-objects", &tx, E2BIG) == 0);
    /* Repeated late failures expose surviving temporary refs; the forwarder's
     * fd stays open until the manager has reported balanced final releases. */
    packet_init(&packet, MAX_OBJECTS);
    for (size_t i = 0; i < packet.count; ++i)
        packet_object(&packet, i, remote_object(h->service, !(i & 1)));
    packet_object(&packet, MAX_OBJECTS - 1, remote_object(UNKNOWN_HANDLE, 1));
    tx = packet_tx(&packet, h->endpoint, BAD);
    for (unsigned i = 0; i < 8; ++i)
        CHECK(rejected_tx(p, "late-rollback-64-objects", &tx, EINVAL) == 0);

    uint64_t data = TOKEN ^ BAD;
    tx = (struct binder_transaction_data){
        .target.handle = UNKNOWN_HANDLE, .code = BAD, .data_size = sizeof(data),
        .data.ptr.buffer = (uintptr_t)&data,
    };
    CHECK(rejected_tx(p, "unknown-target", &tx, EINVAL) == 0);
    tx.target.handle = h->weak;
    CHECK(rejected_tx(p, "weak-only-target", &tx, EOPNOTSUPP) == 0);
    CHECK(rejected(p, "weak-only-acquire", BC_ACQUIRE, &h->weak, sizeof(h->weak), EOPNOTSUPP) == 0);
    CHECK(wait_refs(p, ROLLBACK, 0, 0) == 0 && quiet(p) == 0 && balanced(p, ROLLBACK, 0) == 0);
    printf("SCARLET_BINDER_OBJECTS_REJECT_OK offsets bounds handles weak-only fd flags rollback\n");
    return 0;
}

static int boundary_cases(struct object_peer *p, uint32_t service) {
    phase = "64-objects";
    struct packet packet;
    packet_init(&packet, MAX_OBJECTS);
    for (size_t i = 0; i < MAX_OBJECTS; ++i) {
        struct flat_binder_object object = remote_object(service, !(i & 1));
        object.flags = normal_flags(i);
        packet_object(&packet, i, object);
    }
    CHECK(send_packet(p, BC_TRANSACTION, service, MANY_REQUEST, &packet) == 0);
    CHECK(scalar_reply(p, MANY_REQUEST) == 0);
    CHECK(send_scalar(p, BC_TRANSACTION, service, MANY_REPLY) == 0);
    struct binder_transaction_data reply;
    struct flat_binder_object objects[MAX_OBJECTS];
    CHECK(next_event(p, BR_REPLY, &reply) == 0 && reply.code == MANY_REPLY);
    CHECK(read_packet(p, &reply, MAX_OBJECTS, objects) == 0);
    for (size_t i = 0; i < MAX_OBJECTS; ++i) {
        CHECK(is_remote(objects + i, !(i & 1)) == 0 && objects[i].handle == service);
    }
    /* The previously retained strong/weak pair must survive all 64 temporary
     * attachment refs being dropped, without retaining those refs again. */
    CHECK(free_received(p, &reply) == 0 && call_scalar(p, service, PING) == 0);
    return 0;
}
static int client_main(struct object_peer *p, int report, uint32_t *death_handle) {
    phase = "export-service";
    CHECK(send_scalar(p, BC_TRANSACTION, 0, EXPORT) == 0);
    struct binder_transaction_data reply;
    struct handles h = {0};
    CHECK(next_event(p, BR_REPLY, &reply) == 0 && reply.code == EXPORT);
    CHECK(read_capabilities(p, &reply, &h, 1) == 0);
    phase = "retained-handle";
    /* Also exercise ACQUIRE on an existing strong ref after the receive
     * buffer is gone, and verify paired extra retains cannot release it. */
    CHECK(retain(p, h.service, 1) == 0 && release(p, h.service, 1) == 0);
    CHECK(call_scalar(p, h.service, PING) == 0 && roundtrip(p, &h) == 0);
    CHECK(call_scalar(p, 0, CHECK_LIVE) == 0);
    CHECK(negative_cases(p, &h) == 0 && boundary_cases(p, h.service) == 0);
    phase = "forward-capability";
    struct packet packet;
    packet_init(&packet, 3);
    packet_object(&packet, 0, remote_object(h.service, 1));
    packet_object(&packet, 1, remote_object(h.service, 0));
    packet_object(&packet, 2, remote_object(h.weak, 0));
    CHECK(send_packet(p, BC_TRANSACTION, h.endpoint, FORWARD, &packet) == 0);
    CHECK(scalar_reply(p, FORWARD) == 0 && pipe_byte(report, 'D') == 0);
    CHECK(release(p, h.endpoint, 1) == 0);
    CHECK(call_scalar(p, 0, CHECK_LIVE) == 0);
    printf("SCARLET_BINDER_OBJECTS_TRANSFER_OK export retain call roundtrip third-process max64\n");
    phase = "final-refs";
    CHECK(command(p->fd, BC_RELEASE, &h.service, sizeof(h.service)) == 0);
    CHECK(call_scalar(p, 0, CHECK_WEAK) == 0);
    CHECK(release(p, h.service, 0) == 0 && release(p, h.weak, 0) == 0);
    CHECK(call_scalar(p, 0, CHECK_ZERO) == 0);
    CHECK(quiet(p) == 0 && balanced(p, ROLLBACK, 0) == 0);
    phase = "reexport-for-death";
    CHECK(send_scalar(p, BC_TRANSACTION, 0, REEXPORT) == 0);
    CHECK(next_event(p, BR_REPLY, &reply) == 0 && reply.code == REEXPORT);
    struct flat_binder_object objects[2];
    CHECK(read_packet(p, &reply, 2, objects) == 0);
    CHECK(is_remote(objects, 1) == 0 && is_remote(objects + 1, 0) == 0);
    CHECK(objects[0].handle == objects[1].handle);
    *death_handle = objects[0].handle;
    CHECK(retain(p, *death_handle, 1) == 0 && free_received(p, &reply) == 0);
    CHECK(call_scalar(p, *death_handle, PING) == 0 && call_scalar(p, 0, STOP_OWNER) == 0);
    return 0;
}

static int reap(pid_t *child, int success) {
    int64_t deadline = now_ms() + TIMEOUT_MS;
    while (*child > 0) {
        int status;
        pid_t result = waitpid(*child, &status, WNOHANG);
        if (result == *child) {
            *child = -1;
            if (success && (!WIFEXITED(status) || WEXITSTATUS(status) != 0)) {
                dprintf(STDERR_FILENO, "binder-objects: child status %#x\n", status);
                CHECK(0);
            }
            return 0;
        }
        if (result < 0 && errno == EINTR) continue;
        CHECK(result == 0 && remaining(deadline) > 0);
        CHECK(poll_until(NULL, 0, 10) == 0);
    }
    return 0;
}
static void close_fd(int *fd) {
    if (*fd >= 0) { close(*fd); *fd = -1; }
}
int binder_objects_case(void) {
    printf("SCARLET_BINDER_OBJECTS_BEGIN\n");
    struct object_peer p = {.fd = -1, .pool = MAP_FAILED};
    int ready[2] = {-1, -1}, control[2] = {-1, -1}, report[2] = {-1, -1};
    pid_t manager = -1, forwarder = -1;
    pid_t client = getpid();
    int result = 1;
    struct sigaction ignore = {.sa_handler = SIG_IGN}, old_pipe;
    int restore_signal = 0;
#define RUN(x) do { if (!(x)) { failure(__LINE__, #x); goto cleanup; } } while (0)
    sigemptyset(&ignore.sa_mask);
    RUN(sigaction(SIGPIPE, &ignore, &old_pipe) == 0);
    restore_signal = 1;
    RUN(pipe(ready) == 0);
    manager = fork();
    RUN(manager >= 0);
    if (!manager) {
        close_fd(&ready[0]);
        _exit(manager_main(ready[1], client) == 0 ? 0 : 1);
    }
    close_fd(&ready[1]);
    RUN(pipe_byte(ready[0], 'R') == 0);
    close_fd(&ready[0]);
    RUN(pipe(control) == 0 && pipe(report) == 0);
    forwarder = fork();
    RUN(forwarder >= 0);
    if (!forwarder) {
        close_fd(&control[1]);
        close_fd(&report[0]);
        _exit(forwarder_main(control[0], report[1], client) == 0 ? 0 : 1);
    }
    close_fd(&control[0]);
    close_fd(&report[1]);
    RUN(pipe_byte(report[0], 'R') == 0);
    /* Neither child inherits an open client Binder fd or receive mapping. */
    RUN(peer_open(&p, 1u << ROLLBACK) == 0);
    uint32_t death_handle = 0;
    RUN(client_main(&p, report[0], &death_handle) == 0);
    phase = "dead-nonzero-handle";
    RUN(reap(&manager, 1) == 0);
    RUN(death_handle != 0 && send_scalar(&p, BC_TRANSACTION, death_handle, PING) == 0);
    RUN(next_event(&p, BR_DEAD_REPLY, NULL) == 0);
    RUN(release(&p, death_handle, 1) == 0 && quiet(&p) == 0);
    RUN(write(control[1], "X", 1) == 1 && reap(&forwarder, 1) == 0);
    RUN(peer_close(&p) == 0);
    result = 0;
cleanup:
    if (manager > 0) kill(manager, SIGKILL);
    if (forwarder > 0) kill(forwarder, SIGKILL);
    if (manager > 0) (void)reap(&manager, 0);
    if (forwarder > 0) (void)reap(&forwarder, 0);
    (void)peer_close(&p);
    for (size_t i = 0; i < 2; ++i) {
        close_fd(&ready[i]);
        close_fd(&control[i]);
        close_fd(&report[i]);
    }
    if (restore_signal && sigaction(SIGPIPE, &old_pipe, NULL)) result = 1;
    if (!result)
        printf("SCARLET_BINDER_OBJECTS_OK strong weak retain-before-free owner-roundtrip third-process "
               "offsets4 max64 rollback ref-notifications dead-nonzero-handle\n");
    return result;
#undef RUN
}
