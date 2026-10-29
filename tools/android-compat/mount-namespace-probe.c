/* Namespace FD and native capability refer to one retained namespace object. */
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <linux/capability.h>
#include <sched.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/mount.h>
#include <sys/stat.h>
#include <sys/syscall.h>
#include <sys/wait.h>
#include <unistd.h>
#define NATIVE 0x53430000L
static int fail(const char *step) { printf("SCARLET_NAMESPACE_FAIL %s errno=%d\n", step, errno); return 1; }
static int inner(const char *target) {
    int original = open("/proc/self/ns/mnt", O_RDONLY | O_CLOEXEC);
    long native_original = syscall(NATIVE + 533);
    uint64_t native_id = 0;
    struct stat first, second, current;
    char before[128] = {0}, after[128] = {0};
    if (original < 0 || native_original < 0 || fstat(original, &first) ||
        syscall(NATIVE + 535, native_original, &native_id) || native_id != first.st_ino ||
        readlink("/proc/self/ns/mnt", before, sizeof(before) - 1) < 0) return fail("capture initial identity");
    if (ioctl(original, 0xb703) != CLONE_NEWNS) return fail("namespace type");
    if (unshare(CLONE_NEWNS) || mount(NULL, "/", NULL, MS_PRIVATE | MS_REC, NULL)) return fail("unshare");
    if (mount("tmpfs", target, "tmpfs", MS_NOEXEC | MS_NODEV | MS_NOSUID, "mode=0755,size=1M")) return fail("private mount");
    char marker[256]; snprintf(marker, sizeof(marker), "%s/private-marker", target);
    int fd = open(marker, O_CREAT | O_WRONLY, 0600); if (fd < 0 || close(fd)) return fail("private marker");
    int local = open("/proc/self/ns/mnt", O_RDONLY);
    if (local < 0 || fstat(local, &second) || stat("/proc/self/ns/mnt", &current) ||
        current.st_ino != second.st_ino || second.st_ino == first.st_ino || !S_ISREG(second.st_mode)) return fail("private identity");
    if (setns(original, CLONE_NEWNS) || readlink("/proc/self/ns/mnt", after, sizeof(after) - 1) < 0 ||
        strcmp(before, after)) return fail("restore original namespace");
    if (stat(marker, &current) != -1 || errno != ENOENT) return fail("old namespace view");
    if (setns(local, 0) || stat(marker, &current)) return fail("rejoin retained private view");
    if (syscall(NATIVE + 534, native_original)) return fail("native capability join");
    if (stat(marker, &current) != -1 || errno != ENOENT) return fail("native join view");
    if (setns(local, CLONE_NEWNS)) return fail("rejoin before authority drop");
    struct __user_cap_header_struct header = {_LINUX_CAPABILITY_VERSION_3, 0};
    struct __user_cap_data_struct data[2] = {{0}, {0}};
    if (syscall(__NR_capset, &header, data) || getuid() != 0) return fail("drop authority at uid zero");
    int query = open("/proc/self/ns/mnt", O_RDONLY | O_CLOEXEC);
    if (query < 0) return fail("unprivileged namespace query");
    if (setns(original, CLONE_NEWNS) != -1 || errno != EPERM || stat(marker, &current)) return fail("join denial preserves view");
    if (setns(local, CLONE_NEWUSER) != -1 || errno != EINVAL) return fail("namespace kind validation");
    if (close(query) || close(local) || close(original) || syscall(NATIVE + 102, native_original)) return fail("close capabilities");
    printf("SCARLET_NAMESPACE_OK capture fd native identity join retained-view uid-zero-no-grant\n");
    return 0;
}
int mount_namespace_case(void) {
    char target[128]; snprintf(target, sizeof(target), "/tmp/namespace-%d", getpid());
    if (mkdir(target, 0755)) return fail("mountpoint");
    pid_t child = fork(); if (child < 0) return fail("fork");
    if (!child) _exit(inner(target));
    int status; if (waitpid(child, &status, 0) != child || status != 0) return fail("child result");
    if (rmdir(target)) return fail("remove mountpoint");
    return 0;
}
