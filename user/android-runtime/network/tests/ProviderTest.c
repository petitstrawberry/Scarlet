#include "NetdUpdatablePublic.h"

#include <assert.h>
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <string.h>
#include <sys/socket.h>
#include <unistd.h>

void stub_backend_set_query(uint64_t mask, int error);
void stub_backend_set_open_error(int error);
void stub_backend_reset_calls(void);
unsigned int stub_backend_query_calls(void);
unsigned int stub_backend_open_calls(void);
const char* stub_backend_last_cgroup_path(void);

static void rejects_invalid_cgroup_arguments(void) {
    stub_backend_set_query(0xfe, 0);
    stub_backend_reset_calls();

    assert(libnetd_updatable_init(NULL) == -EINVAL);
    assert(libnetd_updatable_init("") == -EINVAL);
    assert(stub_backend_query_calls() == 0);
    assert(stub_backend_open_calls() == 0);
}

static void propagates_backend_query_failure(void) {
    stub_backend_set_query(0, -EPERM);
    stub_backend_reset_calls();

    assert(libnetd_updatable_init("/sys/fs/cgroup") == -EPERM);
    assert(stub_backend_query_calls() == 1);
    assert(stub_backend_open_calls() == 0);
    assert(libnetd_updatable_tagSocket(-1, 1, 1, 1) == -EPERM);
}

static void rejects_missing_capability_bits(void) {
    stub_backend_set_query(0xfc, 0);
    stub_backend_reset_calls();

    assert(libnetd_updatable_init("/sys/fs/cgroup") == -EOPNOTSUPP);
    assert(stub_backend_query_calls() == 1);
    assert(stub_backend_open_calls() == 0);
}

static void rejects_unexpected_capability_bits(void) {
    stub_backend_set_query(0xff, 0);

    assert(libnetd_updatable_init("/sys/fs/cgroup") == -EOPNOTSUPP);
}

static void propagates_cgroup_open_failure(void) {
    stub_backend_set_query(0xfe, 0);
    stub_backend_set_open_error(-ENOTDIR);

    assert(libnetd_updatable_init("/sys/fs/cgroup") == -ENOTDIR);
    assert(stub_backend_open_calls() == 1);
    assert(strcmp(stub_backend_last_cgroup_path(), "/sys/fs/cgroup") == 0);
}

static void accepts_exact_mask_and_reports_uid_tagging_unsupported(void) {
    stub_backend_set_query(0xfe, 0);
    stub_backend_set_open_error(0);
    stub_backend_reset_calls();

    assert(libnetd_updatable_init("/sys/fs/cgroup") == 0);
    assert(stub_backend_query_calls() == 1);
    assert(stub_backend_open_calls() == 1);

    const int socket_fd = socket(AF_INET, SOCK_DGRAM, 0);
    assert(socket_fd >= 0);
    assert(libnetd_updatable_tagSocket(socket_fd, 0x1234, 10001, 10001) == -EOPNOTSUPP);
    assert(libnetd_updatable_untagSocket(socket_fd) == -EOPNOTSUPP);
    close(socket_fd);

    assert(libnetd_updatable_tagSocket(-1, 0, 0, 0) == -EBADF);
    assert(libnetd_updatable_untagSocket(-1) == -EBADF);

    const int regular_fd = open("/dev/null", O_RDONLY);
    assert(regular_fd >= 0);
    assert(libnetd_updatable_tagSocket(regular_fd, 0, 0, 0) == -ENOTSOCK);
    assert(libnetd_updatable_untagSocket(regular_fd) == -ENOTSOCK);
    close(regular_fd);
}

int main(void) {
    rejects_invalid_cgroup_arguments();
    propagates_backend_query_failure();
    rejects_missing_capability_bits();
    rejects_unexpected_capability_bits();
    propagates_cgroup_open_failure();
    accepts_exact_mask_and_reports_uid_tagging_unsupported();
    puts("netd_updatable provider tests passed");
    return 0;
}

