/* Real Bionic mount/exec/mmap checks in a private native mount namespace. */
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/mount.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <sched.h>
#include <unistd.h>

static int failure(const char *step) {
    printf("SCARLET_MOUNT_FAIL %s errno=%d\n", step, errno);
    return 1;
}
static int denied_exec(const char *path) {
    pid_t child = fork();
    if (child < 0) return failure("fork exec");
    if (!child) {
        char *const args[] = {(char *)path, "mount-unexpected-exec", NULL};
        execv(path, args);
        _exit(errno == EACCES ? 0 : 91);
    }
    int status;
    if (waitpid(child, &status, 0) != child || status != 0) return failure("noexec denial");
    return 0;
}
static int inner(const char *program, const char *target) {
    if (unshare(CLONE_NEWNS)) return failure("unshare mounts");
    unsigned long flags = MS_NOEXEC | MS_NODEV | MS_NOSUID;
    if (mount("tmpfs", target, "tmpfs", flags, "mode=0755,uid=0,gid=0,size=8M")) return failure("mount tmpfs");
    struct stat info;
    if (stat(target, &info) || (info.st_mode & 07777) != 0755 || info.st_uid || info.st_gid) return failure("mount root metadata");
    if (mount("tmpfs", target, "tmpfs", flags | MS_RDONLY, "") != -1 || errno != EOPNOTSUPP) return failure("readonly validation");
    if (mount("tmpfs", target, "tmpfs", flags, "unsupported=1") != -1 || errno != EOPNOTSUPP) return failure("option validation");
    char image[256], script[256], data[256];
    snprintf(image, sizeof(image), "%s/image", target);
    snprintf(script, sizeof(script), "%s/script", target);
    snprintf(data, sizeof(data), "%s/data", target);
    int source = open(program, O_RDONLY), output = open(image, O_CREAT | O_WRONLY, 0755);
    if (source < 0 || output < 0) return failure("open image copy");
    char buffer[4096]; ssize_t count;
    while ((count = read(source, buffer, sizeof(buffer))) > 0)
        if (write(output, buffer, (size_t)count) != count) return failure("copy image");
    if (count < 0 || close(source) || close(output)) return failure("finish image");
    output = open(script, O_CREAT | O_WRONLY, 0755);
    if (output < 0 || write(output, "#!/system/bin/sh\n", 17) != 17 || close(output)) return failure("create script");
    if (denied_exec(image) || denied_exec(script)) return 1;
    int fd = open(data, O_CREAT | O_RDWR, 0600);
    memset(buffer, 0x5a, sizeof(buffer));
    if (fd < 0) return failure("data file");
    for (int page = 0; page < 3; page++)
        if (write(fd, buffer, sizeof(buffer)) != sizeof(buffer)) return failure("data write");
    unsigned char *base = mmap(NULL, 4 * 4096, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (base == MAP_FAILED) return failure("reserve mapping");
    base[0] = 0x33;
    if (mmap(base, 3 * 4096, PROT_READ | PROT_EXEC, MAP_PRIVATE | MAP_FIXED, fd, 0) != MAP_FAILED || errno != EACCES || base[0] != 0x33)
        return failure("fixed executable mmap denial preserves original");
    if (mmap(base, 3 * 4096, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_FIXED, fd, 0) != base || close(fd)) return failure("read-write mapping");
    if (base[0] != 0x5a) return failure("mapped data");
    base[0] = 0xa5;
    if (mprotect(base + 4096, 4096, PROT_READ)) return failure("split mapping");
    if (mprotect(base, 4 * 4096, PROT_READ | PROT_EXEC) != -1 || errno != EACCES) return failure("mixed-range permission ceiling");
    base[3 * 4096] = 0x77; /* A failed mprotect must leave this page writable. */
    pid_t child = fork();
    if (child < 0) return failure("fork mapping");
    if (!child) {
        if (base[0] != 0xa5 || mprotect(base, 4096, PROT_READ | PROT_EXEC) != -1 || errno != EACCES) _exit(92);
        base[0] = 0xbb;
        _exit(0);
    }
    int status;
    if (waitpid(child, &status, 0) != child || status != 0 || base[0] != 0xa5) return failure("fork COW ceiling");
    if (mprotect(base + 3 * 4096, 4096, PROT_READ | PROT_EXEC)) return failure("unrestricted anonymous execution");
    if (munmap(base, 4 * 4096)) return failure("unmap");
    printf("SCARLET_MOUNT_OK tmpfs noexec exec script mmap mprotect atomic fork COW close options\n");
    return 0;
}
int mount_restrictions_case(const char *program) {
    char target[128];
    snprintf(target, sizeof(target), "/tmp/mount-probe-%d", getpid());
    if (mkdir(target, 0755)) return failure("mountpoint");
    pid_t child = fork();
    if (child < 0) return failure("fork namespace");
    if (!child) _exit(inner(program, target));
    int status;
    if (waitpid(child, &status, 0) != child || status != 0) return failure("private mount worker");
    char absent[256]; snprintf(absent, sizeof(absent), "%s/image", target);
    struct stat info;
    if (stat(absent, &info) != -1 || errno != ENOENT) return failure("parent mount isolation");
    if (rmdir(target)) return failure("remove mountpoint");
    return 0;
}
