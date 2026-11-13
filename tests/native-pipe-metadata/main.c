/* Native FileMetadata(302) and the SDK's real C fstat adapter, AArch64. */
#include <errno.h>
#include <fcntl.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

struct raw_metadata {
    uint64_t size;
    uint32_t type, permissions;
    uint64_t created, modified, accessed, id;
    uint32_t links, reserved;
};
_Static_assert(sizeof(struct raw_metadata) == 56, "Native metadata layout");

static uintptr_t metadata(uintptr_t handle, struct raw_metadata *out)
{
    register uintptr_t result __asm__("x0") = handle;
    register uintptr_t pointer __asm__("x1") = (uintptr_t)out;
    register uintptr_t number __asm__("x8") = 302;
    __asm__ volatile("svc #0" : "+r"(result) : "r"(pointer), "r"(number) : "memory");
    return result;
}

#define CHECK(condition) do { \
    if (!(condition)) { \
        fprintf(stderr, "NATIVE_PIPE_METADATA_FAIL line=%d errno=%d\n", __LINE__, errno); \
        return 1; \
    } \
} while (0)

int main(void)
{
    int pipefd[2], other[2], copy, file;
    struct stat reader, writer, duplicate, regular, before;
    struct raw_metadata raw, write_raw, canary, saved;
    char data[5];

    CHECK(pipe(pipefd) == 0);
    CHECK(fstat(pipefd[0], &reader) == 0 && S_ISFIFO(reader.st_mode));
    CHECK(fstat(pipefd[1], &writer) == 0 && S_ISFIFO(writer.st_mode));
    CHECK(reader.st_ino != 0 && reader.st_ino == writer.st_ino);
    CHECK(reader.st_size == 0 && writer.st_size == 0);
    CHECK(metadata(pipefd[0], &raw) == 0 && metadata(pipefd[1], &write_raw) == 0);
    CHECK(raw.type == 5 && write_raw.type == 5 && raw.id == write_raw.id);
    CHECK(raw.permissions == 1 && write_raw.permissions == 2);
    CHECK(raw.created == 0 && raw.modified == 0 && raw.accessed == 0);
    CHECK(raw.links == 0 && raw.reserved == 0);

    copy = dup(pipefd[0]);
    CHECK(copy >= 0 && fstat(copy, &duplicate) == 0);
    CHECK(duplicate.st_ino == reader.st_ino && S_ISFIFO(duplicate.st_mode));
    CHECK(pipe(other) == 0 && metadata(other[0], &raw) == 0);
    CHECK(raw.id != reader.st_ino);
    CHECK(close(other[0]) == 0 && close(other[1]) == 0);

    CHECK(write(pipefd[1], "hello", 5) == 5);
    CHECK(fstat(pipefd[0], &reader) == 0 && reader.st_size == 5);
    CHECK(fstat(pipefd[1], &writer) == 0 && writer.st_size == 5);
    CHECK(read(copy, data, 5) == 5 && memcmp(data, "hello", 5) == 0);
    CHECK(fstat(copy, &duplicate) == 0 && duplicate.st_size == 0);
    CHECK(close(pipefd[1]) == 0);
    CHECK(fstat(copy, &duplicate) == 0 && S_ISFIFO(duplicate.st_mode));
    CHECK(read(copy, data, sizeof(data)) == 0);

    memset(&canary, 0xa5, sizeof(canary));
    saved = canary;
    CHECK(metadata(UINTPTR_MAX, &canary) == UINTPTR_MAX);
    CHECK(memcmp(&canary, &saved, sizeof(canary)) == 0);
    /* Reject a high-word alias of an otherwise valid handle, without truncation. */
    CHECK(metadata((uintptr_t)copy | ((uintptr_t)1 << 32), &canary) == UINTPTR_MAX);
    CHECK(memcmp(&canary, &saved, sizeof(canary)) == 0);
    CHECK(metadata(copy, NULL) == UINTPTR_MAX);
    CHECK(close(pipefd[0]) == 0);
    CHECK(metadata(pipefd[0], &canary) == UINTPTR_MAX);
    CHECK(memcmp(&canary, &saved, sizeof(canary)) == 0);
    /* SDK rejects negative POSIX descriptors as EBADF before the legacy syscall. */
    memset(&reader, 0xa5, sizeof(reader));
    before = reader;
    errno = 0;
    CHECK(fstat(-1, &reader) == -1 && errno == EBADF);
    CHECK(memcmp(&reader, &before, sizeof(reader)) == 0);
    CHECK(close(copy) == 0);

    file = open("/tmp/pipe-metadata-regular", O_CREAT | O_TRUNC | O_RDWR, 0600);
    CHECK(file >= 0 && write(file, "regular", 7) == 7);
    CHECK(fstat(file, &regular) == 0 && S_ISREG(regular.st_mode) && regular.st_size == 7);
    CHECK(metadata(file, &raw) == 0 && raw.type == 0 && raw.size == 7 && raw.id == regular.st_ino);
    CHECK(close(file) == 0 && unlink("/tmp/pipe-metadata-regular") == 0);
    puts("NATIVE_PIPE_METADATA_OK");
    return 0;
}
