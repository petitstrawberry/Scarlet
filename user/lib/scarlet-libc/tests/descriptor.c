#include <errno.h>
#include <fcntl.h>
#include <limits.h>
#include <stddef.h>
#include <unistd.h>

#define CHECK(expr) do { if (!(expr)) return __LINE__; } while (0)

static int path_join(char *output, const char *directory, const char *name) {
    size_t i = 0;
    while (*directory) {
        if (i == PATH_MAX - 1) return 0;
        output[i++] = *directory++;
    }
    if (i == PATH_MAX - 1) return 0;
    output[i++] = '/';
    while (*name) {
        if (i == PATH_MAX - 1) return 0;
        output[i++] = *name++;
    }
    output[i] = '\0';
    return 1;
}

/* The caller supplies a fresh existing absolute directory with the symlink
   descriptor-link -> descriptor-file. Successful output includes
   descriptor-file="abcDEFGH" and descriptor-created="". */
int scarlet_libc_descriptor_test(const char *directory) {
    char path[PATH_MAX];
    char buffer[16];
    CHECK(path_join(path, directory, "descriptor-file"));
    errno = ERANGE;
    int fd = open(path, O_CREAT | O_EXCL | O_RDWR, (mode_t)0760);
    CHECK(fd >= 0 && errno == ERANGE);
    CHECK(write(fd, "abc", 3) == 3);
    CHECK(lseek(fd, 0, SEEK_CUR) == 3);
    CHECK(lseek(fd, -2, SEEK_CUR) == 1);
    CHECK(read(fd, buffer, 2) == 2 && buffer[0] == 'b' && buffer[1] == 'c');
    CHECK(read(fd, buffer, sizeof(buffer)) == 0);
    CHECK(read(fd, NULL, 0) == 0 && write(fd, NULL, 0) == 0);
    CHECK(read(fd, buffer, (size_t)-1) == -1 && errno == EINVAL);
    CHECK(write(fd, buffer, (size_t)-1) == -1 && errno == EINVAL);
    CHECK(read(fd, NULL, 1) == -1 && errno == EFAULT);
    CHECK(write(fd, NULL, 1) == -1 && errno == EFAULT);
    CHECK(lseek(fd, -1, SEEK_SET) == -1 && errno == EINVAL);
    CHECK(lseek(fd, -4, SEEK_CUR) == -1 && errno == EINVAL);
    CHECK(lseek(fd, 0, 19) == -1 && errno == EINVAL);
    CHECK(lseek(fd, 0, SEEK_CUR) == 3);
    CHECK(lseek(fd, LLONG_MAX, SEEK_SET) == LLONG_MAX);
    CHECK(lseek(fd, 1, SEEK_CUR) == -1 && errno == EOVERFLOW);
    CHECK(lseek(fd, 0, SEEK_CUR) == LLONG_MAX);
    CHECK(lseek(fd, 3, SEEK_SET) == 3);

    int copy = dup(fd);
    CHECK(copy >= 0 && copy != fd);
    CHECK(fcntl(fd, F_GETFL) == O_RDWR);
    CHECK(fcntl(fd, F_GETFD) == 0);
    CHECK(fcntl(fd, F_SETFD, FD_CLOEXEC) == 0);
    CHECK(fcntl(fd, F_GETFD) == FD_CLOEXEC);
    CHECK(fcntl(copy, F_GETFD) == 0);
    int cloexec_copy = dup(fd);
    CHECK(cloexec_copy >= 0 && fcntl(cloexec_copy, F_GETFD) == 0);
    CHECK(close(cloexec_copy) == 0);
    CHECK(fcntl(fd, F_SETFD, 0) == 0 && fcntl(fd, F_GETFD) == 0);
    CHECK(lseek(copy, 0, SEEK_SET) == 0);
    CHECK(read(fd, buffer, 1) == 1 && buffer[0] == 'a');
    CHECK(lseek(copy, 0, SEEK_CUR) == 1);
    CHECK(close(copy) == 0);
    CHECK(close(copy) == -1 && errno == EBADF);
    CHECK(read(copy, buffer, 1) == -1 && errno == EBADF);

    int readonly = open(path, O_RDONLY);
    CHECK(readonly >= 0);
    CHECK(write(readonly, "x", 1) == -1 && errno == EBADF);
    CHECK(close(readonly) == 0);
    int writeonly = open(path, O_WRONLY);
    CHECK(writeonly >= 0);
    CHECK(read(writeonly, buffer, 1) == -1 && errno == EBADF);
    CHECK(close(writeonly) == 0);

    CHECK(open(path, O_CREAT | O_EXCL | O_WRONLY, (mode_t)0600) == -1 && errno == EEXIST);
    CHECK(open(path, O_DIRECTORY) == -1 && errno == ENOTDIR);
    CHECK(open(path, O_ACCMODE) == -1 && errno == EINVAL);
    CHECK(open(path, 0x40000000) == -1 && errno == EINVAL);
    CHECK(open(NULL, O_RDONLY) == -1 && errno == EFAULT);

    int base = open(directory, O_RDONLY | O_DIRECTORY);
    CHECK(base >= 0);
    int relative = openat(base, "descriptor-file", O_RDONLY);
    CHECK(relative >= 0);
    CHECK(read(relative, buffer, 3) == 3 && buffer[0] == 'a' && buffer[2] == 'c');
    CHECK(close(relative) == 0);
    CHECK(openat(-1, "descriptor-file", O_RDONLY) == -1 && errno == EBADF);
    CHECK(openat(fd, "child", O_RDONLY) == -1 && errno == ENOTDIR);
    CHECK(openat(base, "missing", O_RDONLY) == -1 && errno == ENOENT);
    CHECK(openat(base, "descriptor-link", O_RDONLY | O_NOFOLLOW) == -1 && errno == ELOOP);
    CHECK(openat(base, "descriptor-link", O_CREAT | O_EXCL | O_WRONLY, (mode_t)0600) == -1 && errno == EEXIST);
    relative = openat(base, "descriptor-link", O_RDONLY);
    CHECK(relative >= 0 && close(relative) == 0);
    int absolute = openat(-1, path, O_RDONLY | O_CLOEXEC);
    CHECK(absolute >= 0 && fcntl(absolute, F_GETFD) == FD_CLOEXEC && close(absolute) == 0);

    /* Separate descriptions must append on every write, even after seek. */
    int append1 = open(path, O_WRONLY | O_APPEND);
    int append2 = openat(base, "descriptor-file", O_WRONLY | O_APPEND);
    CHECK(append1 >= 0 && append2 >= 0);
    CHECK(lseek(append1, 0, SEEK_SET) == 0);
    CHECK(write(append1, "D", 1) == 1);
    CHECK(lseek(append2, 0, SEEK_SET) == 0);
    CHECK(write(append2, "E", 1) == 1);
    CHECK(lseek(append1, 0, SEEK_SET) == 0);
    CHECK(write(append1, "F", 1) == 1);
    CHECK(write(append2, "G", 1) == 1);
    int appendcopy = dup(append1);
    CHECK(appendcopy >= 0);
    CHECK(fcntl(appendcopy, F_GETFL) == (O_WRONLY | O_APPEND));
    CHECK(fcntl(appendcopy, F_SETFL, O_RDONLY) == 0);
    CHECK(fcntl(append1, F_GETFL) == O_WRONLY);
    CHECK(fcntl(appendcopy, F_SETFL, O_APPEND | O_RDWR) == 0);
    CHECK(fcntl(append1, F_GETFL) == (O_WRONLY | O_APPEND));
    CHECK(write(appendcopy, "H", 1) == 1);
    CHECK(close(appendcopy) == 0 && close(append1) == 0 && close(append2) == 0);
    CHECK(lseek(fd, -8, SEEK_END) == 0);
    CHECK(read(fd, buffer, sizeof(buffer)) == 8);
    CHECK(buffer[0] == 'a' && buffer[1] == 'b' && buffer[2] == 'c');
    CHECK(buffer[3] == 'D' && buffer[4] == 'E' && buffer[5] == 'F');
    CHECK(buffer[6] == 'G' && buffer[7] == 'H');
    errno = ERANGE;
    CHECK(close(fd) == 0 && errno == ERANGE);
    CHECK(close(base) == 0);

    CHECK(path_join(path, directory, "descriptor-created"));
    fd = creat(path, (mode_t)0640);
    CHECK(fd >= 0 && write(fd, "discard", 7) == 7 && close(fd) == 0);
    fd = creat(path, (mode_t)0600);
    CHECK(fd >= 0 && lseek(fd, 0, SEEK_END) == 0 && close(fd) == 0);
    /* O_CREAT can also create a read-only open description. */
    CHECK(path_join(path, directory, "descriptor-readonly-created"));
    fd = open(path, O_RDONLY | O_CREAT, (mode_t)0600);
    CHECK(fd >= 0 && read(fd, buffer, 1) == 0);
    CHECK(write(fd, "x", 1) == -1 && errno == EBADF);
    CHECK(close(fd) == 0);
    CHECK(close(-1) == -1 && errno == EBADF);
    CHECK(read(-1, buffer, 1) == -1 && errno == EBADF);
    CHECK(write(-1, buffer, 1) == -1 && errno == EBADF);
    CHECK(lseek(-1, 0, SEEK_SET) == -1 && errno == EBADF);
    CHECK(dup(-1) == -1 && errno == EBADF);
    CHECK(fcntl(-1, F_GETFD) == -1 && errno == EBADF);
    CHECK(fcntl(-1, F_GETFL) == -1 && errno == EBADF);
    return 0;
}
