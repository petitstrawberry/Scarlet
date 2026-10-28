/* SPDX-License-Identifier: MIT */
#include <atomic>
#include <cstdlib>
#include <cstring>
#include <cstdio>
#include <pthread.h>
#include <cstdint>
extern "C" void scarlet_jvm_initialize_runtime(void);
extern "C" char **environ;
static std::atomic<unsigned> destroyed;
struct Lifetime { ~Lifetime() { destroyed.fetch_add(1); } };
static thread_local Lifetime lifetime;
static void* worker(void*) { (void)&lifetime; return nullptr; }
static void poweroff() {
    register uintptr_t x0 asm("x0") = 0, x1 asm("x1") = 0, x8 asm("x8") = 1000;
    asm volatile("svc #0" : "+r"(x0), "+r"(x1), "+r"(x8) : : "memory", "cc", "x2", "x3", "x4", "x5", "x6", "x7", "x9", "x10", "x11", "x12", "x13", "x14", "x15", "x16", "x17", "x30", "v0", "v1", "v2", "v3", "v4", "v5", "v6", "v7", "v8", "v9", "v10", "v11", "v12", "v13", "v14", "v15", "v16", "v17", "v18", "v19", "v20", "v21", "v22", "v23", "v24", "v25", "v26", "v27", "v28", "v29", "v30", "v31");
}
#define CHECK(x) do { if (!(x)) { printf("OPENJDK_RUNTIME FAIL: %s\n", #x); poweroff(); return 1; } } while(0)
int main() {
    scarlet_jvm_initialize_runtime();
    bool has_home = false;
    for (char **p = environ; p && *p; ++p) if (!strcmp(*p, "HOME=/root")) has_home = true;
    CHECK(has_home);
    CHECK(getenv("HOME") && !strcmp(getenv("HOME"), "/root"));
    pthread_t threads[4];
    for (auto& t : threads) CHECK(pthread_create(&t, nullptr, worker, nullptr) == 0);
    for (auto& t : threads) CHECK(pthread_join(t, nullptr) == 0);
    CHECK(destroyed.load() == 4);
    puts("OPENJDK_RUNTIME PASS (environment and C++ TLS destructors)");
    poweroff();
    return 0;
}
