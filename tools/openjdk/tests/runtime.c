/* SPDX-License-Identifier: MIT */
#include <errno.h>
#include <ffi.h>
#include <pthread.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/mman.h>
#include <time.h>
#include <unistd.h>

static uintptr_t native_call3(uintptr_t number, uintptr_t a, uintptr_t b, uintptr_t c) {
    register uintptr_t x0 asm("x0") = a, x1 asm("x1") = b, x2 asm("x2") = c, x8 asm("x8") = number;
    asm volatile("svc #0" : "+r"(x0), "+r"(x1), "+r"(x2), "+r"(x8) : :
        "memory", "cc", "x3", "x4", "x5", "x6", "x7", "x9", "x10", "x11",
        "x12", "x13", "x14", "x15", "x16", "x17", "x30",
        "v0", "v1", "v2", "v3", "v4", "v5", "v6", "v7", "v8", "v9", "v10", "v11",
        "v12", "v13", "v14", "v15", "v16", "v17", "v18", "v19", "v20", "v21",
        "v22", "v23", "v24", "v25", "v26", "v27", "v28", "v29", "v30", "v31");
    return x0;
}
static uintptr_t native_call(uintptr_t number, uintptr_t a, uintptr_t b) {
    return native_call3(number, a, b, 0);
}
static void diagnostic(const char* text) {
    while (*text) native_call(16, (unsigned char)*text++, 0);
}
#define CHECK(x) do { if (!(x)) { char message[512]; snprintf(message, sizeof(message), "OPENJDK_RUNTIME FAIL line=%d: %s errno=%d\n", __LINE__, #x, errno); diagnostic(message); native_call(1000, 0, 0); exit(1); } } while (0)

static pthread_key_t key;
static pthread_mutex_t mutex = PTHREAD_MUTEX_INITIALIZER;
static pthread_cond_t ready = PTHREAD_COND_INITIALIZER;
static unsigned workers_ready;
static int release_workers;
static double ffi_target(uint64_t a, double b, uint64_t c, double d) {
    return (double)(a + c) + b + d;
}
static void ffi_check(void) {
    ffi_cif cif;
    ffi_type *types[] = { &ffi_type_uint64, &ffi_type_double, &ffi_type_uint64, &ffi_type_double };
    uint64_t a = 17, c = 31;
    double b = 0.25, d = 0.5, result = 0;
    void *args[] = { &a, &b, &c, &d };
    CHECK(ffi_prep_cif(&cif, FFI_DEFAULT_ABI, 4, &ffi_type_double, types) == FFI_OK);
    ffi_call(&cif, FFI_FN(ffi_target), &result, args);
    CHECK(result == 48.75);
}
struct fault_context {
    uint64_t version, size, address, syndrome, registers[31], sp, pc, nzcv;
};
static unsigned faults;
static uintptr_t fault_address;
static void fault_handler(struct fault_context *c) {
    CHECK(c->version == 1 && c->size == 304 && c->address == fault_address);
    CHECK(((c->syndrome >> 26) & 63) == 0x24);
    CHECK(native_call3(645, 0, 0, 0) == UINTPTR_MAX);
    CHECK(native_call(646, (uintptr_t)c + 8, 0) == UINTPTR_MAX);
    uint64_t pc = c->pc;
    c->pc = UINT64_MAX;
    CHECK(native_call(646, (uintptr_t)c, 0) == UINTPTR_MAX);
    c->pc = pc + 4; // Skip the deliberate LDR fault.
    if (c->address == 0) c->registers[10] = 1234;
    c->nzcv |= 0x3ff; // Privileged PSTATE bits must never be restored.
    ++faults;
    asm volatile("movi v0.16b, #0" ::: "v0");
    native_call(646, (uintptr_t)c, 0);
    CHECK(0 && "fault return must resume interrupted code");
}
static void fault_check(void) {
    CHECK(native_call(646, 0, 0) == UINTPTR_MAX);
    char *stack = mmap(NULL, 32768, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    CHECK(stack != MAP_FAILED);
    CHECK(native_call3(645, (uintptr_t)fault_handler + 1, (uintptr_t)stack, 32768) == UINTPTR_MAX);
    CHECK(native_call3(645, (uintptr_t)fault_handler, (uintptr_t)stack, 32768) == 0);
    for (int i = 0; i < 2; ++i) {
        uint64_t value, vector, flags, preserved;
        const uint64_t marker = UINT64_C(0x3ff23456789abcde);
        asm volatile("fmov d0, %x[marker]\nmov x9, #0\nmov x10, #17\nmov x12, #99\n"
                     "cmp x10, #17\nldr x10, [x9]\ncset %x[flags], eq\n"
                     "mov %x[value], x10\nfmov %x[vector], d0\nmov %x[preserved], x12"
            : [value] "=&r"(value), [vector] "=&r"(vector), [flags] "=&r"(flags),
              [preserved] "=&r"(preserved)
            : [marker] "r"(marker) : "x9", "x10", "x12", "v0", "memory", "cc");
        CHECK(value == 1234 && vector == marker && flags == 1 && preserved == 99);
    }
    CHECK(faults == 2);
    char *guard = mmap(NULL, 4096, PROT_NONE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    CHECK(guard != MAP_FAILED);
    fault_address = (uintptr_t)guard;
    asm volatile("str xzr, [%0]" :: "r"(guard) : "memory");
    CHECK(faults == 3);
    CHECK(mprotect(guard, 4096, PROT_READ) == 0);
    asm volatile("str xzr, [%0]" :: "r"(guard) : "memory");
    CHECK(faults == 4);
    CHECK(munmap(guard, 4096) == 0);
    CHECK(native_call3(645, 0, 0, 0) == 0);
    CHECK(munmap(stack, 32768) == 0);
    puts("native fault recovery: GPR, SIMD, NZCV and context validation PASS");
}
static void* worker(void* value) {
    CHECK(pthread_setspecific(key, value) == 0);
    errno = (int)(uintptr_t)value;
    uintptr_t bounds[2], marker;
    CHECK(native_call(703, (uintptr_t)&marker, (uintptr_t)bounds) == 0);
    CHECK(bounds[0] <= (uintptr_t)&marker && (uintptr_t)&marker < bounds[1]);
    ffi_check();
    CHECK(pthread_mutex_lock(&mutex) == 0);
    ++workers_ready;
    CHECK(pthread_cond_broadcast(&ready) == 0);
    while (!release_workers) CHECK(pthread_cond_wait(&ready, &mutex) == 0);
    CHECK(pthread_mutex_unlock(&mutex) == 0);
    CHECK(pthread_getspecific(key) == value);
    CHECK(errno == (int)(uintptr_t)value);
    return value;
}
int main(void) {
    diagnostic("OPENJDK_RUNTIME START\n");
    CHECK(puts("native C stdout") >= 0);
    fault_check();
    struct { uint64_t page_size, total, free; } memory;
    CHECK(native_call(53, (uintptr_t)&memory, sizeof(memory)) == sizeof(memory));
    CHECK(memory.page_size == 4096 && memory.total > 0 && memory.free <= memory.total);
    CHECK(native_call(53, 0, sizeof(memory)) == UINTPTR_MAX);
    CHECK(native_call(53, (uintptr_t)&memory, sizeof(memory)-1) == UINTPTR_MAX);
    printf("memory pages: total=%lu free=%lu\n", memory.total, memory.free);
    char* p = mmap(NULL, 3 * 4096, PROT_NONE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    CHECK(p != MAP_FAILED);
    CHECK(mprotect(p, 3 * 4096, PROT_READ | PROT_WRITE) == 0);
    p[0] = 41; p[4096] = 42; p[8192] = 43;
    CHECK(mprotect(p+4096, 4096, PROT_NONE) == 0);
    CHECK(mprotect(p+4096, 4096, PROT_READ | PROT_WRITE) == 0);
    CHECK(p[0] == 41 && p[4096] == 42 && p[8192] == 43);
    CHECK(mmap(p+4096, 4096, PROT_NONE, MAP_FIXED | MAP_PRIVATE | MAP_ANONYMOUS, -1, 0) == p+4096);
    CHECK(mprotect(p+4096, 4096, PROT_READ | PROT_WRITE) == 0);
    CHECK(p[0] == 41 && p[4096] == 0 && p[8192] == 43);
    CHECK(munmap(p, 3 * 4096) == 0);
    uintptr_t word = UINT64_C(0x12345678fedcba98);
    CHECK(native_call3(704, (uintptr_t)&word, 8, 91) == word);
    CHECK(native_call3(704, (uintptr_t)&word, 4, 91) == UINT32_C(0xfedcba98));
    CHECK(native_call3(704, (uintptr_t)&word+1, 8, 91) == 91);
    CHECK(native_call3(704, (uintptr_t)&word, 3, 91) == 91);
    CHECK(native_call3(704, 0, 8, 91) == 91);
    CHECK(native_call3(704, UINTPTR_MAX-7, 8, 91) == 91);
    CHECK(native_call3(704, (uintptr_t)p, 8, 91) == 91);
    p = mmap(NULL, 4096, PROT_NONE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    CHECK(p != MAP_FAILED);
    CHECK(native_call3(704, (uintptr_t)p, 8, 91) == 91);
    CHECK(mprotect(p, 4096, PROT_READ | PROT_WRITE) == 0);
    /* The first read must also work before userspace faults in a lazy page. */
    CHECK(native_call3(704, (uintptr_t)p, 8, 91) == 0);
    *(uintptr_t*)p = 147;
    CHECK(native_call3(704, (uintptr_t)p, 8, 91) == 147);
    CHECK(munmap(p, 4096) == 0);
    ffi_check();
    CHECK(pthread_key_create(&key, NULL) == 0);
    pthread_t threads[4];
    for (uintptr_t i=0; i<4; ++i) CHECK(pthread_create(&threads[i], NULL, worker, (void*)(i+101)) == 0);
    CHECK(pthread_mutex_lock(&mutex) == 0);
    while (workers_ready != 4) CHECK(pthread_cond_wait(&ready, &mutex) == 0);
    release_workers = 1;
    CHECK(pthread_cond_broadcast(&ready) == 0);
    CHECK(pthread_mutex_unlock(&mutex) == 0);
    for (uintptr_t i=0; i<4; ++i) {
        void* result = NULL;
        CHECK(pthread_join(threads[i], &result) == 0);
        CHECK(result == (void*)(i+101));
    }
    CHECK(pthread_key_delete(key) == 0);
    diagnostic("OPENJDK_RUNTIME PASS (native prerequisites; JVM execution is separate)\n");
    native_call(1000, 0, 0);
    return 0;
}
