/* Native mount-event relationships exercised through stock Bionic syscalls. */
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <sched.h>
#include <stdio.h>
#include <string.h>
#include <sys/mount.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <unistd.h>

static int fail(const char *step) {
    printf("SCARLET_PROPAGATION_FAIL %s errno=%d\n", step, errno);
    return 1;
}
static int token(int fd, int write_token) {
    char byte = 'x';
    return (write_token ? write(fd, &byte, 1) : read(fd, &byte, 1)) == 1 ? 0 : -1;
}
static int marker(const char *path) {
    int fd = open(path, O_CREAT | O_WRONLY, 0600);
    if (fd < 0) return -1;
    return close(fd);
}
static int stacking(const char *target) {
    char covered[256], source[256], lower[256], config[256], visible[256];
    snprintf(covered, sizeof(covered), "%s/cover", target);
    snprintf(source, sizeof(source), "%s/cover/bootstrap", target);
    snprintf(lower, sizeof(lower), "%s/cover/lower", target);
    snprintf(config, sizeof(config), "%s/cover/bootstrap/config", target);
    snprintf(visible, sizeof(visible), "%s/cover/config", target);
    if (mkdir(covered, 0755) || mount("tmpfs", covered, "tmpfs", MS_NOEXEC | MS_NODEV | MS_NOSUID, "size=1M")) return fail("cover base");
    if (marker(lower) || mkdir(source, 0755) || marker(config)) return fail("cover data");
    int fd = open(lower, O_RDONLY); struct stat before, after;
    if (fd < 0 || fstat(fd, &before) || mount(source, covered, NULL, MS_BIND | MS_REC, NULL)) return fail("cover bind");
    if (stat(visible, &after) || stat(lower, &after) != -1 || errno != ENOENT ||
        fstat(fd, &after) || before.st_ino != after.st_ino) return fail("cover visibility or retained fd");
    errno = 0;
    if (umount2(covered, MNT_FORCE) != -1 || errno != EOPNOTSUPP) return fail("unsupported force unmount");
    if (umount2(covered, MNT_DETACH) || stat(lower, &after) || stat(config, &after)) return fail("uncover lower");
    if (mount("tmpfs", covered, "tmpfs", MS_NODEV, "size=1M")) return fail("regular overmount");
    if (stat(lower, &after) != -1 || errno != ENOENT || umount2(covered, MNT_DETACH) || stat(lower, &after)) return fail("regular uncover");
    if (close(fd) || umount2(covered, MNT_DETACH)) return fail("cover cleanup");
    printf("SCARLET_MOUNT_STACK_OK bind regular retained-fd lazy-unmount\n");
    return 0;
}
static int inner(const char *target) {
    /* The outer controller's View is never connected to these test groups. */
    if (unshare(CLONE_NEWNS) || mount(NULL, "/", NULL, MS_PRIVATE | MS_REC, NULL)) return fail("isolate view");
    if (mount("tmpfs", target, "tmpfs", MS_NOEXEC | MS_NODEV | MS_NOSUID, "mode=0755,size=8M")) return fail("mount base");
    if (stacking(target)) return 1;
    char up[256], local[256], source[256], alias[256], nested[256], deep[256], seed[256];
    snprintf(up, sizeof(up), "%s/up", target); snprintf(local, sizeof(local), "%s/local", target);
    snprintf(source, sizeof(source), "%s/source", target); snprintf(alias, sizeof(alias), "%s/alias", target);
    snprintf(nested, sizeof(nested), "%s/source/nested", target);
    snprintf(deep, sizeof(deep), "%s/source/nested/deep", target);
    snprintf(seed, sizeof(seed), "%s/source/nested/seed", target);
    if (mkdir(up, 0755) || mkdir(local, 0755) || mkdir(source, 0755) || mkdir(alias, 0755) || mkdir(nested, 0755)) return fail("directories");
    if (mount("tmpfs", nested, "tmpfs", MS_NODEV | MS_NOSUID | MS_NOEXEC, "mode=0755,size=1M") || mkdir(deep, 0755) || marker(seed)) return fail("nested source");
    if (mount(NULL, target, NULL, MS_SHARED | MS_REC, NULL)) return fail("make shared");
    int commands[2], events[2];
    if (pipe(commands) || pipe(events)) return fail("pipes");
    pid_t worker = fork(); if (worker < 0) return fail("fork");
    if (!worker) {
        close(commands[1]); close(events[0]);
        if (unshare(CLONE_NEWNS) || mount(NULL, target, NULL, MS_SLAVE | MS_REC, NULL) ||
            mount(NULL, target, NULL, MS_SHARED | MS_REC, NULL)) _exit(fail("downstream shared view"));
        if (token(events[1], 1) || token(commands[0], 0)) _exit(fail("upstream handshake"));
        char path[256]; struct stat info;
        snprintf(path, sizeof(path), "%s/up/from-parent", target);
        if (stat(path, &info)) _exit(fail("receive upstream mount"));
        if (mount("tmpfs", local, "tmpfs", MS_NOEXEC | MS_NODEV | MS_NOSUID, "mode=0755,size=1M")) _exit(fail("local mount"));
        snprintf(path, sizeof(path), "%s/local/only-worker", target);
        if (marker(path) || mount(source, alias, NULL, MS_BIND | MS_REC, NULL)) _exit(fail("recursive bind"));
        snprintf(path, sizeof(path), "%s/alias/nested/seed", target);
        if (stat(path, &info)) _exit(fail("bind copied submount"));
        snprintf(path, sizeof(path), "%s/alias/nested/deep", target);
        if (mount("tmpfs", path, "tmpfs", MS_NOEXEC | MS_NODEV | MS_NOSUID, "mode=0755,size=1M")) _exit(fail("nested local mount"));
        snprintf(path, sizeof(path), "%s/alias/nested/deep/only-worker", target);
        if (marker(path) || token(events[1], 1) || token(commands[0], 0)) _exit(fail("local handshake"));
        _exit(0);
    }
    close(commands[0]); close(events[1]);
    if (token(events[0], 0)) return fail("worker ready");
    if (mount("tmpfs", up, "tmpfs", MS_NOEXEC | MS_NODEV | MS_NOSUID, "mode=0755,size=1M")) return fail("upstream mount");
    char path[256]; struct stat info;
    snprintf(path, sizeof(path), "%s/up/from-parent", target);
    if (marker(path) || token(commands[1], 1) || token(events[0], 0)) return fail("upstream publish");
    snprintf(path, sizeof(path), "%s/local/only-worker", target);
    if (stat(path, &info) != -1 || errno != ENOENT) return fail("local mount flowed upstream");
    snprintf(path, sizeof(path), "%s/source/nested/deep/only-worker", target);
    if (stat(path, &info) != -1 || errno != ENOENT) return fail("nested mount flowed upstream");
    if (token(commands[1], 1)) return fail("finish worker");
    int status; if (waitpid(worker, &status, 0) != worker || status != 0) return fail("worker exit");
    close(commands[1]); close(events[0]);
    printf("SCARLET_PROPAGATION_OK shared slave recursive namespace bind no-backflow\n");
    return 0;
}
int mount_propagation_case(void) {
    char target[128]; snprintf(target, sizeof(target), "/tmp/propagation-%d", getpid());
    if (mkdir(target, 0755)) return fail("mountpoint");
    pid_t child = fork(); if (child < 0) return fail("fork controller");
    if (!child) _exit(inner(target));
    int status; if (waitpid(child, &status, 0) != child || status != 0) return fail("controller exit");
    if (rmdir(target)) return fail("remove private mountpoint");
    return 0;
}
