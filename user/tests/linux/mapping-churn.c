#define _GNU_SOURCE
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <sys/mman.h>
#include <time.h>
#include <unistd.h>

static uint64_t now(void) {
    struct timespec time;
    if (clock_gettime(CLOCK_MONOTONIC, &time)) return 0;
    return (uint64_t)time.tv_sec * 1000000000 + time.tv_nsec;
}
int main(void) {
    size_t page = (size_t)sysconf(_SC_PAGESIZE), size = 8 * 1024 * 1024;
    unsigned char *region = mmap(NULL, size + 2 * page, PROT_READ | PROT_WRITE,
                                MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (region == MAP_FAILED) { perror("mmap"); return 1; }
    unsigned char *middle = region + page, *right = middle + size;
    memset(region, 0x31, page);
    memset(right, 0x73, page);
    uint64_t elapsed = 0;
    for (unsigned round = 0; round < 4; ++round) {
        for (size_t offset = 0; offset < size; offset += page) {
            if (middle[offset] || middle[offset + page - 1]) {
                puts("FAIL: replacement anonymous pages retain stale data"); return 1;
            }
            middle[offset] = 0xa5;
            middle[offset + page - 1] = 0x5a;
        }
        uint64_t started = now();
        if (munmap(middle, size)) { perror("munmap"); return 1; }
        elapsed += now() - started;
        for (size_t offset = 0; offset < page; ++offset) {
            if (region[offset] != 0x31 || right[offset] != 0x73) {
                puts("FAIL: partial unmap changed a neighboring page"); return 1;
            }
        }
        void *replacement = mmap(middle, size, PROT_READ | PROT_WRITE,
                                 MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED, -1, 0);
        if (replacement != middle) { perror("fixed replacement mmap"); return 1; }
    }
    if (munmap(region, size + 2 * page)) { perror("final munmap"); return 1; }
    printf("PASS: four 8 MiB partial unmaps, preserved neighbors and zeroed replacements; unmap time %llu ms\n",
           (unsigned long long)(elapsed / 1000000));
    return 0;
}
