#include "Backend.h"

#include <errno.h>
#include <fcntl.h>
#include <stdint.h>
#include <string.h>
#include <unistd.h>

static uint64_t g_query_mask;
static int g_query_error;
static int g_open_error;
static unsigned int g_query_calls;
static unsigned int g_open_calls;
static char g_last_cgroup_path[256];

void stub_backend_set_query(uint64_t mask, int error) {
    g_query_mask = mask;
    g_query_error = error;
}

void stub_backend_set_open_error(int error) {
    g_open_error = error;
}

void stub_backend_reset_calls(void) {
    g_query_calls = 0;
    g_open_calls = 0;
    g_last_cgroup_path[0] = '\0';
}

unsigned int stub_backend_query_calls(void) {
    return g_query_calls;
}

unsigned int stub_backend_open_calls(void) {
    return g_open_calls;
}

const char* stub_backend_last_cgroup_path(void) {
    return g_last_cgroup_path;
}

int netd_updatable_backend_query_traffic(uint64_t* supported_operations) {
    ++g_query_calls;
    if (g_query_error < 0) return g_query_error;
    *supported_operations = g_query_mask;
    return 0;
}

int netd_updatable_backend_open_cgroup(const char* path, int* fd) {
    ++g_open_calls;
    if (path == NULL || strlen(path) >= sizeof(g_last_cgroup_path)) return -EINVAL;
    strcpy(g_last_cgroup_path, path);
    if (g_open_error < 0) return g_open_error;

    *fd = open("/dev/null", O_RDONLY | O_CLOEXEC);
    return *fd == -1 ? -errno : 0;
}

