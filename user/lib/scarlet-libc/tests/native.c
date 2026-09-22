#include <stdlib.h>
#include <errno.h>
#include <limits.h>
#include <fcntl.h>
#include <sys/stat.h>
#include <unistd.h>

#define CHECK(expr) do { if (!(expr)) return __LINE__; } while (0)
_Static_assert(sizeof(struct timespec) == 16, "native 64-bit timespec layout");

static int equal(const char *a, const char *b) {
    while (*a && *a == *b) { a++; b++; }
    return *a == *b;
}

/* Run with cwd at the Rust fixture root. Rust verifies resulting metadata. */
int scarlet_libc_probe(int file, int directory, const char *expected) {
    char buffer[PATH_MAX];
    CHECK(realpath("alias/../file", buffer) == buffer);
    CHECK(equal(buffer, expected));
    char *allocated = realpath("alias/../file", NULL);
    CHECK(allocated != NULL && equal(allocated, expected));
    free(allocated);
    CHECK(realpath("", buffer) == NULL && errno == ENOENT);
    CHECK(realpath("missing", buffer) == NULL && errno == ENOENT);
    CHECK(realpath("dangling", buffer) == NULL && errno == ENOENT);
    CHECK(realpath("loop", buffer) == NULL && errno == ELOOP);
    CHECK(realpath("real/file/..", buffer) == NULL && errno == ENOTDIR);
    CHECK(realpath("real/file/.", buffer) == NULL && errno == ENOTDIR);
    CHECK(realpath("file-link/", buffer) == NULL && errno == ENOTDIR);
    CHECK(realpath(NULL, buffer) == NULL && errno == EINVAL);

    unsigned char *p = calloc(32, 1);
    CHECK(p != NULL && (size_t)p % 16 == 0);
    for (int i = 0; i < 32; i++) CHECK(p[i] == 0);
    p[0] = 42;
    p = realloc(p, 64);
    CHECK(p != NULL && p[0] == 42);
    CHECK(realloc(p, (size_t)-1) == NULL && errno == ENOMEM);
    CHECK(p[0] == 42);
    free(p);

    /* Repeated small, odd-sized requests exercise allocator split alignment,
       preservation through realloc, and reuse of fragmented free blocks. */
    unsigned char *blocks[96];
    for (int round = 0; round < 4; round++) {
        for (int i = 0; i < 96; i++) {
            size_t size = (size_t)i * 2 + 1;
            blocks[i] = malloc(size);
            CHECK(blocks[i] != NULL && (size_t)blocks[i] % 16 == 0);
            for (size_t j = 0; j < size; j++) blocks[i][j] = (unsigned char)(i + j);
        }
        for (int i = 1; i < 96; i += 2) { free(blocks[i]); blocks[i] = NULL; }
        for (int i = 0; i < 96; i += 2) {
            size_t size = (size_t)i * 2 + 1;
            unsigned char *grown = realloc(blocks[i], size + 137);
            CHECK(grown != NULL);
            for (size_t j = 0; j < size; j++) CHECK(grown[j] == (unsigned char)(i + j));
            blocks[i] = grown;
        }
        for (int i = 0; i < 96; i++) free(blocks[i]);
    }

    CHECK(futimens(file, NULL) == 0);
    struct timespec times[2] = {{-1, UTIME_NOW}, {-1, UTIME_NOW}};
    CHECK(utimensat(AT_FDCWD, "real/file", times, 0) == 0);
    times[0] = (struct timespec){1234, 999999999};
    times[1] = (struct timespec){5678, 123456789};
    /* An absolute path must ignore the otherwise invalid directory fd. */
    CHECK(utimensat(-1, expected, times, 0) == 0);
    CHECK(utimensat(directory, "real/file", times, 0) == 0);
    times[0].tv_nsec = UTIME_OMIT;
    times[1].tv_sec = 6789;
    CHECK(futimens(file, times) == 0);
    CHECK(fsync(file) == 0);
    CHECK(fdatasync(file) == 0);
    times[1].tv_nsec = 1000000000;
    CHECK(futimens(file, times) == -1 && errno == EINVAL);
    CHECK(fsync(-1) == -1 && errno == EBADF);
    times[1].tv_nsec = 0;
    CHECK(utimensat(-1, "real/file", times, 0) == -1 && errno == EBADF);
    CHECK(utimensat(directory, "real/file", times, 0x8000) == -1 && errno == EINVAL);
    CHECK(utimensat(file, "child", times, 0) == -1 && errno == ENOTDIR);
    times[0].tv_sec = 4321;
    times[0].tv_nsec = 0;
    times[1].tv_sec = 8765;
    CHECK(utimensat(directory, "file-link", times, AT_SYMLINK_NOFOLLOW) == 0);
    return 0;
}
