#include <errno.h>
#include <fcntl.h>
#include <limits.h>
#include <stdio.h>
#include <string.h>
#include <sys/file.h>
#include <unistd.h>

#define CHECK(x) do { if (!(x)) return __LINE__; } while (0)

int scarlet_libc_positioned_test(const char *directory) {
    char path[PATH_MAX];
    char bytes[64];
    CHECK(snprintf(path, sizeof path, "%s/positioned-file", directory) > 0);
    int fd = open(path, O_RDWR | O_CREAT | O_EXCL, 0600U);
    CHECK(fd >= 0);
    CHECK(write(fd, "0123456789abcdef", 16) == 16);
    CHECK(lseek(fd, 40, SEEK_SET) == 40);
    int duplicate = dup(fd);
    CHECK(duplicate >= 0);
    errno = 701;
    CHECK(pread(fd, bytes, 4, 3) == 4 && memcmp(bytes, "3456", 4) == 0);
    CHECK(errno == 701);
    CHECK(lseek(duplicate, 0, SEEK_CUR) == 40);
    CHECK(fcntl(fd, F_SETFL, O_APPEND) == 0);
    CHECK(pwrite(duplicate, "XY", 2, 1) == 2);
    CHECK(lseek(fd, 0, SEEK_CUR) == 40);
    CHECK(pread(fd, bytes, 4, 0) == 4 && memcmp(bytes, "0XY3", 4) == 0);
    CHECK(pread(fd, bytes, 1, 16) == 0);
    CHECK(pread(fd, 0, 0, 0) == 0 && pwrite(fd, 0, 0, 0) == 0);
    CHECK(pread(fd, bytes, 1, -1) == -1 && errno == EINVAL);
    CHECK(pwrite(fd, bytes, 1, LLONG_MAX) == -1 && errno == EOVERFLOW);
    CHECK(pread(fd, 0, 1, 0) == -1 && errno == EFAULT);
    CHECK(pwrite(-1, bytes, 1, 0) == -1 && errno == EBADF);
    CHECK(ftruncate(fd, -1) == -1 && errno == EINVAL);
    int huge = ftruncate(fd, LLONG_MAX);
    CHECK(huge == 0 || (huge == -1 && (errno == EOVERFLOW || errno == ENOSPC || errno == ENOMEM)));
    CHECK(lseek(duplicate, 0, SEEK_CUR) == 40);
    CHECK(pread(fd, bytes, 16, 0) == 16 && memcmp(bytes, "0XY3456789abcdef", 16) == 0);
    if (huge == 0) CHECK(ftruncate(fd, 16) == 0);
    errno = 702;
    CHECK(ftruncate(fd, 3) == 0 && errno == 702);
    CHECK(lseek(duplicate, 0, SEEK_CUR) == 40);
    CHECK(ftruncate(duplicate, 16) == 0);
    CHECK(lseek(fd, 0, SEEK_CUR) == 40);
    CHECK(pread(fd, bytes, 16, 0) == 16 && memcmp(bytes, "0XY", 3) == 0);
    for (int i = 3; i < 16; i++) CHECK(bytes[i] == 0);
    int reader = open(path, O_RDONLY);
    int writer = open(path, O_WRONLY);
    CHECK(reader >= 0 && writer >= 0);
    CHECK(ftruncate(reader, 0) == -1 && errno == EBADF);
    CHECK(pwrite(reader, bytes, 1, 0) == -1 && errno == EBADF);
    CHECK(pread(writer, bytes, 1, 0) == -1 && errno == EBADF);
    CHECK(close(writer) == 0);
    CHECK(flock(fd, LOCK_EX) == -1 && errno == ENOTSUP);
    CHECK(flock(fd, LOCK_SH | LOCK_EX | LOCK_NB) == -1 && errno == EINVAL);
    CHECK(flock(fd, LOCK_SH | LOCK_NB) == 0);
    CHECK(flock(reader, LOCK_SH | LOCK_NB) == 0);
    CHECK(flock(fd, LOCK_EX | LOCK_NB) == -1 && errno == EWOULDBLOCK);
    CHECK(flock(reader, LOCK_UN) == 0);
    CHECK(flock(fd, LOCK_EX | LOCK_NB) == 0);
    CHECK(flock(reader, LOCK_EX | LOCK_NB) == -1 && errno == EWOULDBLOCK);
    CHECK(close(fd) == 0);
    CHECK(flock(reader, LOCK_EX | LOCK_NB) == -1 && errno == EWOULDBLOCK);
    CHECK(close(duplicate) == 0);
    CHECK(flock(reader, LOCK_EX | LOCK_NB) == 0);
    CHECK(flock(reader, LOCK_UN) == 0);
    CHECK(close(reader) == 0);
    CHECK(fsync(-1) == -1 && errno == EBADF);
    return 0;
}
