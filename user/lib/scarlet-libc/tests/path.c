#include <errno.h>
#include <fcntl.h>
#include <limits.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

#define CHECK(expr) do { if (!(expr)) return __LINE__; } while (0)

/* Caller supplies directory containing empty path-dir, path-nonempty/child,
   path-link -> path-target, path-dangling -> path-missing, and
   path-dir-link -> path-dir. Files and links must be fresh for each call. */
int scarlet_libc_path_test(const char *directory, int ext2) {
    char path[PATH_MAX], target[PATH_MAX], buffer[PATH_MAX];
    CHECK(snprintf(target, sizeof(target), "%s/path-target", directory) > 0);
    int fd = open(target, O_CREAT | O_EXCL | O_WRONLY, (mode_t)0600);
    CHECK(fd >= 0 && write(fd, "keep", 4) == 4 && close(fd) == 0);
    CHECK(snprintf(path, sizeof(path), "%s/path-link/", directory) > 0);
    CHECK(unlink(path) == -1 && errno == ENOTDIR);
    CHECK(rmdir(path) == -1 && errno == ENOTDIR);
    CHECK(snprintf(path, sizeof(path), "%s/path-link", directory) > 0);
    CHECK(rmdir(path) == -1 && errno == ENOTDIR);
    errno = ERANGE;
    CHECK(unlink(path) == 0 && errno == ERANGE);
    CHECK(unlink(path) == -1 && errno == ENOENT);
    fd = open(target, O_RDONLY);
    CHECK(fd >= 0 && read(fd, buffer, sizeof(buffer)) == 4);
    CHECK(memcmp(buffer, "keep", 4) == 0 && close(fd) == 0);
    CHECK(snprintf(path, sizeof(path), "%s/path-dangling", directory) > 0);
    CHECK(unlink(path) == 0);
    CHECK(unlink(path) == -1 && errno == ENOENT);
    CHECK(snprintf(path, sizeof(path), "%s/path-dir-link/", directory) > 0);
    CHECK(rmdir(path) == -1 && errno == ENOTDIR);
    CHECK(snprintf(path, sizeof(path), "%s/path-dir-link", directory) > 0);
    CHECK(unlink(path) == 0);
    CHECK(snprintf(path, sizeof(path), "%s/path-nonempty", directory) > 0);
    CHECK(unlink(path) == -1 && errno == EISDIR);
    CHECK(rmdir(path) == -1 && errno == ENOTEMPTY);
    CHECK(snprintf(path, sizeof(path), "%s/path-nonempty/child", directory) > 0);
    fd = open(path, O_RDONLY);
    CHECK(fd >= 0 && close(fd) == 0);
    CHECK(unlink(path) == 0);
    CHECK(snprintf(path, sizeof(path), "%s/path-nonempty/", directory) > 0);
    int removed_directory = rmdir(path);
    CHECK(ext2 ? (removed_directory == -1 && errno == EOPNOTSUPP) : removed_directory == 0);
    if (removed_directory != 0) {
        fd = open(path, O_RDONLY | O_DIRECTORY);
        CHECK(fd >= 0 && close(fd) == 0);
    }
    CHECK(snprintf(path, sizeof(path), "%s/path-dir/.", directory) > 0);
    CHECK(rmdir(path) == -1 && errno == EINVAL);
    CHECK(snprintf(path, sizeof(path), "%s/path-dir///", directory) > 0);
    errno = EOVERFLOW;
    removed_directory = rmdir(path);
    CHECK(ext2 ? (removed_directory == -1 && errno == EOPNOTSUPP) :
                 (removed_directory == 0 && errno == EOVERFLOW));
    if (removed_directory != 0) {
        fd = open(path, O_RDONLY | O_DIRECTORY);
        CHECK(fd >= 0 && close(fd) == 0);
    }
    CHECK(snprintf(path, sizeof(path), "%s/path-target/", directory) > 0);
    CHECK(unlink(path) == -1 && errno == ENOTDIR);
    CHECK(rmdir(target) == -1 && errno == ENOTDIR);
    /* ext2 currently rejects last-link removal while a description is open;
       tmpfs keeps the unlinked node alive. Both must preserve the live fd. */
    fd = open(target, O_RDONLY);
    CHECK(fd >= 0);
    int duplicate = dup(fd);
    CHECK(duplicate >= 0 && close(fd) == 0);
    int removed = unlink(target);
    CHECK(ext2 ? (removed == -1 && errno == EBUSY) : removed == 0);
    CHECK(read(duplicate, buffer, sizeof(buffer)) == 4 && memcmp(buffer, "keep", 4) == 0);
    if (removed == 0) {
        fd = open(target, O_CREAT | O_EXCL | O_WRONLY, (mode_t)0600);
        CHECK(fd >= 0 && write(fd, "new", 3) == 3 && close(fd) == 0);
        CHECK(lseek(duplicate, 0, SEEK_SET) == 0);
        CHECK(read(duplicate, buffer, sizeof(buffer)) == 4 && memcmp(buffer, "keep", 4) == 0);
    } else {
        CHECK(unlink(target) == -1 && errno == EBUSY);
    }
    CHECK(close(duplicate) == 0 && unlink(target) == 0);
    CHECK(unlink("") == -1 && errno == ENOENT);
    CHECK(rmdir("/") == -1 && errno == EBUSY);
    CHECK(unlink(NULL) == -1 && errno == EFAULT);
    CHECK(rmdir(NULL) == -1 && errno == EFAULT);

    memset(buffer, 0x5a, sizeof(buffer));
    CHECK(getcwd(buffer, 1) == NULL && errno == ERANGE);
    CHECK(buffer[0] == 0x5a && buffer[1] == 0x5a);
    CHECK(getcwd(buffer, 0) == NULL && errno == EINVAL);
    CHECK(getcwd(NULL, 1) == NULL && errno == ERANGE);
    errno = EOVERFLOW;
    CHECK(getcwd(buffer, sizeof(buffer)) == buffer && errno == EOVERFLOW);
    CHECK(buffer[0] == '/');
    char *allocated = getcwd(NULL, 0);
    CHECK(allocated != NULL && strcmp(allocated, buffer) == 0 && errno == EOVERFLOW);
    free(allocated);
    allocated = getcwd(NULL, strlen(buffer) + 1);
    CHECK(allocated != NULL && strcmp(allocated, buffer) == 0);
    free(allocated);
    CHECK(getcwd(target, strlen(buffer)) == NULL && errno == ERANGE);
    CHECK(getcwd(target, strlen(buffer) + 1) == target && strcmp(target, buffer) == 0);
    return 0;
}
