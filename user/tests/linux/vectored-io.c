#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/uio.h>
#include <unistd.h>

#define PAGE 4096
static int failures;
static void check(int condition, const char *name) {
    printf("%s: %s\n", condition ? "PASS" : "FAIL", name);
    failures += !condition;
}
/* Touch virtual neighbors in a different order, with live guard pages between
 * their physical allocations. A syscall must resolve each page separately. */
static unsigned char *fragmented(void) {
    unsigned char *memory = mmap(NULL, 9 * PAGE, PROT_READ | PROT_WRITE,
                                 MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (memory == MAP_FAILED) { perror("mmap"); exit(2); }
    const int order[] = {0, 4, 1, 5, 2, 6, 3, 7, 8};
    for (unsigned i = 0; i < sizeof(order)/sizeof(order[0]); ++i)
        memset(memory + order[i] * PAGE, 0x55, PAGE);
    return memory;
}
static unsigned char pattern(size_t index) { return (index * 37 + index / 113) & 255; }
static int intact(unsigned char *memory) {
    for (size_t i = 4 * PAGE; i < 9 * PAGE; ++i) if (memory[i] != 0x55) return 0;
    return 1;
}
int main(void) {
    unsigned char *source = fragmented(), *destination = fragmented(), *metadata = fragmented();
    const size_t first = PAGE + 31, second = PAGE + 19, length = first + second;
    for (size_t i = 0; i < length; ++i) source[17 + i] = pattern(i);
    /* The first iovec record itself straddles a page boundary. */
    struct iovec *vectors = (struct iovec *)(metadata + PAGE - 8);
    struct iovec gather[] = {{NULL, 0}, {source + 17, first}, {source + 17 + first, second}};
    memcpy(vectors, gather, sizeof(gather));
    char path[] = "/tmp/linux-vectored-io-XXXXXX";
    int file = mkstemp(path);
    if (file < 0) { perror("mkstemp"); return 2; }
    check(writev(file, vectors, 3) == (ssize_t)length, "writev gathers fragmented buffers and split metadata");
    lseek(file, 0, SEEK_SET);
    unsigned char *verification = malloc(length);
    ssize_t got = read(file, verification, length);
    int correct = got == (ssize_t)length;
    for (size_t i = 0; correct && i < length; ++i) correct = verification[i] == pattern(i);
    check(correct, "writev file bytes preserve vector order");
    struct iovec scatter[] = {{destination + 23, first}, {NULL, 0}, {destination + 23 + first, second}};
    memcpy(vectors, scatter, sizeof(scatter));
    lseek(file, 0, SEEK_SET);
    got = readv(file, vectors, 3);
    correct = got == (ssize_t)length;
    for (size_t i = 0; correct && i < length; ++i) correct = destination[23 + i] == pattern(i);
    check(correct, "readv scatters across separately backed virtual pages");
    check(intact(source) && intact(destination) && intact(metadata), "adjacent physical guard pages remain intact");
    check(readv(file, vectors, 3) == 0, "readv reports EOF");
    lseek(file, 0, SEEK_SET);
    scatter[0].iov_len = length + 99;
    scatter[1].iov_len = scatter[2].iov_len = 0;
    memcpy(vectors, scatter, sizeof(scatter));
    check(readv(file, vectors, 3) == (ssize_t)length, "readv returns a short read at EOF");
    errno = 0;
    check(readv(file, (const struct iovec *)(uintptr_t)1, 1) == -1 && errno == EFAULT,
          "unmapped iovec metadata returns EFAULT");
    struct iovec overflow[] = {{source, SIZE_MAX}};
    errno = 0;
    check(writev(file, overflow, 1) == -1 && errno == EINVAL, "invalid signed vector length returns EINVAL");
    lseek(file, 0, SEEK_SET);
    check(mprotect(destination + 8 * PAGE, PAGE, PROT_READ) == 0, "prepare read-only destination");
    struct iovec readonly = {destination + 8 * PAGE, 1};
    errno = 0;
    check(readv(file, &readonly, 1) == -1 && errno == EFAULT && lseek(file, 0, SEEK_CUR) == 0,
          "readv rejects read-only output before consuming file bytes");
    check(readv(file, NULL, 0) == 0, "empty vector list succeeds");
    int pipes[2];
    if (pipe2(pipes, O_NONBLOCK) == 0) {
        struct iovec empty = {verification, 1};
        errno = 0;
        check(readv(pipes[0], &empty, 1) == -1 && errno == EAGAIN, "empty nonblocking pipe returns EAGAIN");
        struct iovec words[] = {{"hello", 5}, {"world", 5}};
        check(writev(pipes[1], words, 2) == 10 && read(pipes[0], verification, 10) == 10 &&
              !memcmp(verification, "helloworld", 10), "small pipe writev uses one ordered stream write");
        close(pipes[0]); close(pipes[1]);
    } else { check(0, "create nonblocking pipe"); }
    close(file); unlink(path); free(verification);
    munmap(source, 9 * PAGE); munmap(destination, 9 * PAGE); munmap(metadata, 9 * PAGE);
    return failures != 0;
}
