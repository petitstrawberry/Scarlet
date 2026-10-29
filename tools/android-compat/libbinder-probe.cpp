// Exercise stock Android libbinder/libutils; no ioctl shim or library patch.
#include <binder/Binder.h>
#include <binder/IPCThreadState.h>
#include <binder/Parcel.h>
#include <binder/ProcessState.h>
#include <utils/String16.h>
#include <android/log.h>
#include <dlfcn.h>
#include <poll.h>
#include <signal.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>
#include <cerrno>
#include <cstdio>
#include <cstring>

using namespace android;
constexpr uint32_t GET = IBinder::FIRST_CALL_TRANSACTION;
constexpr uint32_t STATS = GET + 1;
constexpr uint32_t RETURN = GET + 2;
constexpr uint32_t ECHO = IBinder::FIRST_CALL_TRANSACTION;
constexpr int CYCLES = 2;
static int created;
static int destroyed;
static pid_t client_pid;

static int failure(const char* expression, int line) {
    dprintf(2, "SCARLET_LIBBINDER_FAIL pid=%d line=%d %s errno=%d\n",
            getpid(), line, expression, errno);
    return 1;
}
#define CHECK(x) do { if (!(x)) return failure(#x, __LINE__); } while (0)

static const String16& descriptor() {
    static const String16 value("scarlet.compat.Lifetime");
    return value;
}

class Service : public BBinder {
public:
    Service() { ++created; }
    ~Service() override { ++destroyed; }
    const String16& getInterfaceDescriptor() const override { return descriptor(); }
protected:
    status_t onTransact(uint32_t code, const Parcel& data, Parcel* reply, uint32_t flags) override {
        if (code != ECHO) return BBinder::onTransact(code, data, reply, flags);
        if (!data.enforceInterface(descriptor()) ||
            IPCThreadState::self()->getCallingPid() != client_pid) return PERMISSION_DENIED;
        int32_t value = 0;
        if (data.readInt32(&value) != OK || data.readString16() != String16("Scarlet Binder") ||
            data.dataAvail() != 0) return BAD_VALUE;
        status_t status = reply->writeInt32(value + 1);
        return status == OK ? reply->writeString16(String16("stock libbinder")) : status;
    }
};

class Root : public BBinder {
    wp<Service> last;
public:
    const String16& getInterfaceDescriptor() const override { return descriptor(); }
protected:
    status_t onTransact(uint32_t code, const Parcel& data, Parcel* reply, uint32_t flags) override {
        if (code != GET && code != STATS && code != RETURN)
            return BBinder::onTransact(code, data, reply, flags);
        if (!data.enforceInterface(descriptor()) ||
            IPCThreadState::self()->getCallingPid() != client_pid) return PERMISSION_DENIED;
        if (code == GET) {
            if (created != destroyed) return INVALID_OPERATION;
            auto service = sp<Service>::make();
            last = service;
            return reply->writeStrongBinder(service);
        }
        if (code == RETURN) {
            sp<IBinder> returned, null;
            if (data.readStrongBinder(&returned) != OK ||
                data.readNullableStrongBinder(&null) != OK || null != nullptr ||
                data.dataAvail() != 0 || returned == nullptr || returned->localBinder() == nullptr ||
                returned != last.promote()) return BAD_VALUE;
            return reply->writeInt32(1);
        }
        status_t status = reply->writeInt32(destroyed);
        if (status != OK) return status;
        return reply->writeInt32(last.get_refs() ? last.get_refs()->getWeakCount() : -1);
    }
};

class Recipient : public IBinder::DeathRecipient {
    wp<IBinder> expected;
public:
    int calls = 0;
    bool matched = false;
    explicit Recipient(const sp<IBinder>& binder) : expected(binder) {}
    void binderDied(const wp<IBinder>& who) override {
        ++calls;
        matched = who == expected;
    }
};

static int pump(int fd) {
    struct pollfd event = {fd, POLLIN, 0};
    int count;
    do { count = poll(&event, 1, 5000); } while (count < 0 && errno == EINTR);
    CHECK(count == 1 && event.revents == POLLIN);
    CHECK(IPCThreadState::self()->handlePolledCommands() == OK);
    return 0;
}

static int server(int ready, int stop) {
    client_pid = getppid();
    auto process = ProcessState::self();
    CHECK(process->setThreadPoolMaxThreadCount(0) == OK);
    IPCThreadState::self()->setTheContextObject(sp<Root>::make());
    CHECK(process->becomeContextManager());
    int fd = -1;
    CHECK(IPCThreadState::self()->setupPolling(&fd) == OK);
    dprintf(1, "SCARLET_LIBBINDER_SERVER_READY\n");
    CHECK(write(ready, "R", 1) == 1);
    for (;;) {
        struct pollfd events[2] = {{fd, POLLIN, 0}, {stop, POLLIN, 0}};
        int count;
        do { count = poll(events, 2, 5000); } while (count < 0 && errno == EINTR);
        CHECK(count > 0);
        if (events[1].revents & POLLIN) {
            char value = 0;
            CHECK(read(stop, &value, 1) == 1 && value == 'S');
            break;
        }
        CHECK(!(events[1].revents & (POLLHUP | POLLERR | POLLNVAL)));
        CHECK(events[0].revents & POLLIN);
        CHECK(IPCThreadState::self()->handlePolledCommands() == OK);
    }
    // A final service remains referenced when its serving process exits.
    CHECK(created == CYCLES + 1 && destroyed == CYCLES);
    return 0;
}

static int stats(const sp<IBinder>& root, int deaths, int weak_count) {
    Parcel request, reply;
    CHECK(request.writeInterfaceToken(descriptor()) == OK);
    CHECK(root->transact(STATS, request, &reply) == OK);
    int32_t actual_deaths = -1, actual_weak = -1;
    CHECK(reply.readInt32(&actual_deaths) == OK && reply.readInt32(&actual_weak) == OK);
    CHECK(actual_deaths == deaths && actual_weak == weak_count && reply.dataAvail() == 0);
    return 0;
}

static int client(int ready, int stop) {
    struct pollfd event = {ready, POLLIN, 0};
    int count;
    do { count = poll(&event, 1, 5000); } while (count < 0 && errno == EINTR);
    CHECK(count > 0 && (event.revents & POLLIN));
    char value = 0;
    CHECK(read(ready, &value, 1) == 1 && value == 'R');
    auto root = ProcessState::self()->getContextObject(nullptr);
    CHECK(root != nullptr && root->remoteBinder() != nullptr);
    dprintf(1, "SCARLET_LIBBINDER_CONTEXT_OK\n");
    for (int cycle = 1; cycle <= CYCLES; ++cycle) {
        sp<IBinder> service;
        {
            Parcel request, reply;
            CHECK(request.writeInterfaceToken(descriptor()) == OK);
            CHECK(root->transact(GET, request, &reply) == OK);
            CHECK(reply.readStrongBinder(&service) == OK && reply.dataAvail() == 0);
            CHECK(service != nullptr && service->remoteBinder() != nullptr);
        }
        IPCThreadState::self()->flushCommands();
        dprintf(1, "SCARLET_LIBBINDER_CYCLE %d HANDLE_OK\n", cycle);
        {
            Parcel request, reply;
            CHECK(request.writeInterfaceToken(descriptor()) == OK);
            CHECK(request.writeStrongBinder(service) == OK && request.writeStrongBinder(nullptr) == OK);
            CHECK(root->transact(RETURN, request, &reply) == OK);
            CHECK(reply.readInt32() == 1 && reply.dataAvail() == 0);
        }
        dprintf(1, "SCARLET_LIBBINDER_CYCLE %d ROUNDTRIP_OK\n", cycle);
        {
            Parcel request, reply;
            CHECK(request.writeInterfaceToken(descriptor()) == OK);
            CHECK(request.writeInt32(40 + cycle) == OK);
            CHECK(request.writeString16(String16("Scarlet Binder")) == OK);
            CHECK(service->transact(ECHO, request, &reply) == OK);
            CHECK(reply.readInt32() == 41 + cycle);
            CHECK(reply.readString16() == String16("stock libbinder") && reply.dataAvail() == 0);
        }
        dprintf(1, "SCARLET_LIBBINDER_CYCLE %d CALL_OK\n", cycle);
        {
            Parcel request, reply;
            CHECK(service->transact(0x00ffffff, request, &reply) == UNKNOWN_TRANSACTION);
        }
        dprintf(1, "SCARLET_LIBBINDER_CYCLE %d STATUS_OK\n", cycle);
        wp<IBinder> weak = service;
        service.clear();
        IPCThreadState::self()->flushCommands();
        // One owner wp<> and one driver weak hold remain after BBinder dies.
        CHECK(stats(root, cycle, 2) == 0);
        dprintf(1, "SCARLET_LIBBINDER_CYCLE %d STRONG_ZERO_WEAK_LIVE\n", cycle);
        weak.clear();
        IPCThreadState::self()->flushCommands();
        CHECK(stats(root, cycle, 1) == 0);
        dprintf(1, "SCARLET_LIBBINDER_CYCLE %d REFS_RELEASED\n", cycle);
    }
    sp<IBinder> watched;
    {
        Parcel request, reply;
        CHECK(request.writeInterfaceToken(descriptor()) == OK);
        CHECK(root->transact(GET, request, &reply) == OK);
        CHECK(reply.readStrongBinder(&watched) == OK && watched != nullptr);
    }
    int fd = -1;
    CHECK(IPCThreadState::self()->setupPolling(&fd) == OK);
    auto cancelled = sp<Recipient>::make(watched);
    auto active = sp<Recipient>::make(watched);
    const int weak_before = watched->getWeakRefs()->getWeakCount();
    CHECK(watched->linkToDeath(cancelled) == OK);
    CHECK(watched->unlinkToDeath(cancelled) == OK);
    CHECK(pump(fd) == 0); // BR_CLEAR_DEATH_NOTIFICATION_DONE releases proxy hold
    CHECK(cancelled->calls == 0 && watched->getWeakRefs()->getWeakCount() == weak_before);
    dprintf(1, "SCARLET_LIBBINDER_DEATH_UNLINK_OK\n");
    CHECK(watched->linkToDeath(active) == OK);
    root.clear();
    IPCThreadState::self()->flushCommands();
    CHECK(write(stop, "S", 1) == 1);
    CHECK(pump(fd) == 0); // BR_DEAD_BINDER; libbinder emits CLEAR and DEAD_DONE
    CHECK(active->calls == 1 && active->matched && cancelled->calls == 0);
    dprintf(1, "SCARLET_LIBBINDER_DEATH_CALLBACK_OK\n");
    CHECK(pump(fd) == 0); // CLEAR_DONE must follow DEAD_DONE
    CHECK(watched->getWeakRefs()->getWeakCount() == weak_before);
    dprintf(1, "SCARLET_LIBBINDER_DEATH_CLEAR_OK\n");
    {
        Parcel request, reply;
        CHECK(watched->transact(ECHO, request, &reply) == DEAD_OBJECT);
        CHECK(watched->linkToDeath(cancelled) == DEAD_OBJECT);
    }
    dprintf(1, "SCARLET_LIBBINDER_DEAD_OBJECT_OK\n");
    struct pollfd idle = {fd, POLLIN, 0};
    do { count = poll(&idle, 1, 50); } while (count < 0 && errno == EINTR);
    CHECK(count == 0 && cancelled->calls == 0 && active->calls == 1);
    dprintf(1, "SCARLET_LIBBINDER_DEATH_QUIET_OK\n");
    return 0;
}

int main() {
    __android_log_set_logger([](const struct __android_log_message* message) {
        dprintf(2, "LIBBINDER_LOG %s: %s\n", message->tag, message->message);
    });
    dprintf(1, "SCARLET_LIBBINDER_BEGIN\n");
    Dl_info library = {};
    CHECK(dladdr(reinterpret_cast<const void*>(&ProcessState::self), &library) != 0);
    CHECK(library.dli_fname && strcmp(library.dli_fname, "/system/lib64/libbinder.so") == 0);
    dprintf(1, "SCARLET_LIBBINDER_RUNTIME /system/lib64/libbinder.so\n");
    int ready[2], stop[2];
    CHECK(pipe(ready) == 0 && pipe(stop) == 0);
    // Fork before creating either process's ProcessState singleton.
    pid_t pid = fork();
    CHECK(pid >= 0);
    if (pid == 0) {
        close(ready[0]); close(stop[1]);
        _exit(server(ready[1], stop[0]));
    }
    close(ready[1]); close(stop[0]);
    int result = client(ready[0], stop[1]);
    if (result) kill(pid, SIGKILL);
    int status = -1;
    pid_t waited;
    do { waited = waitpid(pid, &status, 0); } while (waited < 0 && errno == EINTR);
    close(ready[0]); close(stop[1]);
    CHECK(waited == pid && !result && WIFEXITED(status) && WEXITSTATUS(status) == 0);
    dprintf(1, "SCARLET_LIBBINDER_SERVER_EXIT 0\n");
    dprintf(1, "SCARLET_LIBBINDER_OK\n");
    return 0;
}
