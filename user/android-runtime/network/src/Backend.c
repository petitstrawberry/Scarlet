#include "Backend.h"

#include <errno.h>
#include <fcntl.h>
#include <stddef.h>
#include <sys/stat.h>
#include <sys/syscall.h>
#include <sys/vfs.h>
#include <unistd.h>

#define NATIVE_SYSCALL_BASE 0x53430000L
#define NETWORK_TRAFFIC_CONFIGURE_V1 922

#define TRAFFIC_CONFIGURE_VERSION_V1 1u
#define TRAFFIC_CONFIGURE_QUERY 0u

#define CGROUP2_SUPER_MAGIC_VALUE 0x63677270UL

struct NetworkTrafficConfigureV1 {
    uint32_t version;
    uint32_t operation;
    uint64_t remaining;
    uint8_t subject[32];
    uint8_t meter[32];
};

_Static_assert(sizeof(struct NetworkTrafficConfigureV1) == 80,
               "NativeTrafficConfigureV1 must match the Scarlet ABI");
_Static_assert(offsetof(struct NetworkTrafficConfigureV1, remaining) == 8,
               "NativeTrafficConfigureV1 remaining offset");
_Static_assert(offsetof(struct NetworkTrafficConfigureV1, subject) == 16,
               "NativeTrafficConfigureV1 subject offset");
_Static_assert(offsetof(struct NetworkTrafficConfigureV1, meter) == 48,
               "NativeTrafficConfigureV1 meter offset");

int netd_updatable_backend_query_traffic(uint64_t* supported_operations) {
    if (supported_operations == NULL) return -EINVAL;

    const struct NetworkTrafficConfigureV1 request = {
            .version = TRAFFIC_CONFIGURE_VERSION_V1,
            .operation = TRAFFIC_CONFIGURE_QUERY,
            .remaining = 0,
            .subject = {0},
            .meter = {0},
    };

    errno = 0;
    const long result = syscall(NATIVE_SYSCALL_BASE + NETWORK_TRAFFIC_CONFIGURE_V1, &request);
    if (result == -1) return errno == 0 ? -EIO : -errno;
    if (result < 0) return (int)result;

    *supported_operations = (uint64_t)result;
    return 0;
}

int netd_updatable_backend_open_cgroup(const char* path, int* fd) {
    if (path == NULL || path[0] != '/' || path[1] == '\0' || fd == NULL) return -EINVAL;

    const int candidate = open(path, O_DIRECTORY | O_RDONLY | O_CLOEXEC);
    if (candidate == -1) return -errno;

    struct statfs filesystem;
    if (fstatfs(candidate, &filesystem) == -1) {
        const int error = errno;
        close(candidate);
        return -error;
    }
    if ((unsigned long)filesystem.f_type != CGROUP2_SUPER_MAGIC_VALUE) {
        close(candidate);
        return -EINVAL;
    }

    *fd = candidate;
    return 0;
}

