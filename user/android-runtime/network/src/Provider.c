#include "NetdUpdatablePublic.h"

#include "Backend.h"

#include <errno.h>
#include <pthread.h>
#include <stdbool.h>
#include <stdint.h>
#include <sys/socket.h>
#include <unistd.h>

#if defined(__GNUC__)
#define NETD_PUBLIC __attribute__((visibility("default")))
#else
#define NETD_PUBLIC
#endif

/* Operations 1 through 7: meter/link/global configuration and removal. */
#define REQUIRED_TRAFFIC_OPERATIONS 0xfeULL

struct ProviderState {
    bool initialized;
    int cgroup_fd;
    uint64_t supported_operations;
};

static pthread_mutex_t g_state_lock = PTHREAD_MUTEX_INITIALIZER;
static struct ProviderState g_state = {
        .initialized = false,
        .cgroup_fd = -1,
        .supported_operations = 0,
};

static bool is_initialized(void) {
    bool initialized;
    pthread_mutex_lock(&g_state_lock);
    initialized = g_state.initialized;
    pthread_mutex_unlock(&g_state_lock);
    return initialized;
}

/* Match the public contract's socket validation before reporting that UID tagging is absent. */
static int validate_socket(int sock_fd) {
    struct sockaddr_storage address;
    socklen_t length = sizeof(address);
    if (getsockname(sock_fd, (struct sockaddr*)&address, &length) == -1) return -errno;
    return 0;
}

NETD_PUBLIC int libnetd_updatable_init(const char* cg2_path) {
    if (cg2_path == NULL || cg2_path[0] == '\0') return -EINVAL;

    uint64_t supported_operations = 0;
    int error = netd_updatable_backend_query_traffic(&supported_operations);
    if (error < 0) return error;
    if (supported_operations != REQUIRED_TRAFFIC_OPERATIONS) return -EOPNOTSUPP;

    int cgroup_fd = -1;
    error = netd_updatable_backend_open_cgroup(cg2_path, &cgroup_fd);
    if (error < 0) return error;

    if (pthread_mutex_lock(&g_state_lock) != 0) {
        close(cgroup_fd);
        return -EIO;
    }
    const int previous_fd = g_state.cgroup_fd;
    g_state.initialized = true;
    g_state.cgroup_fd = cgroup_fd;
    g_state.supported_operations = supported_operations;
    pthread_mutex_unlock(&g_state_lock);

    if (previous_fd >= 0) close(previous_fd);
    return 0;
}

NETD_PUBLIC int libnetd_updatable_tagSocket(int sock_fd, uint32_t tag, uid_t charge_uid,
                                            uid_t real_uid) {
    (void)tag;
    (void)charge_uid;
    (void)real_uid;
    if (!is_initialized()) return -EPERM;

    const int error = validate_socket(sock_fd);
    if (error < 0) return error;
    return -EOPNOTSUPP;
}

NETD_PUBLIC int libnetd_updatable_untagSocket(int sock_fd) {
    if (!is_initialized()) return -EPERM;

    const int error = validate_socket(sock_fd);
    if (error < 0) return error;
    return -EOPNOTSUPP;
}

