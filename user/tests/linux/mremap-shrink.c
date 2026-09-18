#define _GNU_SOURCE
#include <errno.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <sys/mman.h>
#include <unistd.h>

int main(void) {
    size_t page = (size_t)sysconf(_SC_PAGESIZE);
    size_t reserved = 16 * 1024 * 1024 + sizeof(int);
    size_t used = 2 * 1024 * 1024 + 37;
    size_t retained = (used + page - 1) & ~(page - 1);
    size_t total = (reserved + page - 1) & ~(page - 1);
    for (unsigned round = 0; round < 4; ++round) {
        unsigned char *memory = mmap(NULL, reserved, PROT_READ | PROT_WRITE,
                                     MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
        if (memory == MAP_FAILED) { perror("mmap"); return 1; }
        memset(memory, 0x51 + round, retained);
        memory[total - 1] = 0x93;
        int flags = round & 1 ? MREMAP_MAYMOVE : 0;
        if (mremap(memory, reserved, used, flags) != memory) {
            perror("in-place mremap shrink"); return 1;
        }
        for (size_t i = 0; i < retained; ++i) {
            if (memory[i] != 0x51 + round) {
                puts("FAIL: shrink changed a retained byte"); return 1;
            }
        }
        /* This hint must be available now, without MAP_FIXED replacing a VMA.
           Reading the replacement also detects stale page-table translations. */
        unsigned char *tail = mmap(memory + retained, total - retained,
                                   PROT_READ | PROT_WRITE,
                                   MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
        if (tail != memory + retained) {
            puts("FAIL: removed tail is not available for a new mapping"); return 1;
        }
        if (tail[0] || tail[total - retained - 1]) {
            puts("FAIL: replacement mapping retained stale data"); return 1;
        }
        tail[0] = 0x42;
        if (mremap(memory, used, retained - 1, 0) != memory
            || memory[retained - 1] != 0x51 + round || tail[0] != 0x42) {
            puts("FAIL: same-page resize changed a neighboring mapping"); return 1;
        }
        errno = 0;
        if (mremap(memory + 1, used, page, 0) != MAP_FAILED || errno != EINVAL) {
            puts("FAIL: unaligned address accepted"); return 1;
        }
        errno = 0;
        if (mremap(memory, used, 0, 0) != MAP_FAILED || errno != EINVAL) {
            puts("FAIL: zero new size accepted"); return 1;
        }
        errno = 0;
        if (mremap(memory, used, page, 8) != MAP_FAILED || errno != EINVAL) {
            puts("FAIL: unknown flags accepted"); return 1;
        }
        if (munmap(memory, retained) || munmap(tail, total - retained)) {
            perror("cleanup munmap"); return 1;
        }
    }
    puts("PASS: four 16 MiB mremap shrinks, retained data, reclaimed tails, page rounding and invalid arguments");
    return 0;
}
