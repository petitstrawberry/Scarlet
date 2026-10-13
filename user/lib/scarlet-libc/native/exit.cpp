// Process and DSO finalizers for the native C/C++ ABI. Allocation, mutexes,
// streams and immediate termination remain owned by the Rust libc backend.
#include <pthread.h>
#include <stdio.h>
#include <stdlib.h>

namespace {
struct ExitEntry {
    void (*function)(void*);
    void* argument;
    void* dso;
    ExitEntry* next;
};
pthread_mutex_t exitMutex = PTHREAD_MUTEX_INITIALIZER;
ExitEntry* entries = nullptr;

bool traceFinalizers() {
    return getenv("SCARLET_LIBC_DIAGNOSTICS") != nullptr;
}
}

extern "C" int __cxa_atexit(void (*function)(void*), void* argument, void* dso) {
    auto* entry = static_cast<ExitEntry*>(malloc(sizeof(ExitEntry)));
    if (!entry) return -1;
    *entry = { function, argument, dso, nullptr };
    if (pthread_mutex_lock(&exitMutex) != 0) {
        free(entry);
        return -1;
    }
    entry->next = entries;
    entries = entry;
    pthread_mutex_unlock(&exitMutex);
    return 0;
}

extern "C" void __cxa_finalize(void* dso) {
    const bool trace = traceFinalizers();
    for (;;) {
        if (pthread_mutex_lock(&exitMutex) != 0) abort();
        ExitEntry** cursor = &entries;
        while (*cursor && dso && (*cursor)->dso != dso) cursor = &(*cursor)->next;
        ExitEntry* entry = *cursor;
        if (entry) *cursor = entry->next;
        pthread_mutex_unlock(&exitMutex);
        if (!entry) return;
        auto function = entry->function;
        auto argument = entry->argument;
        free(entry);
        // Remove before calling: a recursive finalize cannot call it twice.
        if (trace) {
            printf("SCARLET_LIBC_FINALIZER_BEGIN fn=%p arg=%p\n",
                reinterpret_cast<void*>(function), argument);
            fflush(stdout);
        }
        function(argument);
        if (trace) {
            printf("SCARLET_LIBC_FINALIZER_END fn=%p arg=%p\n",
                reinterpret_cast<void*>(function), argument);
            fflush(stdout);
        }
    }
}

extern "C" int atexit(void (*function)()) {
    struct Plain { void (*function)(); };
    auto* entry = static_cast<Plain*>(malloc(sizeof(Plain)));
    if (!entry) return -1;
    entry->function = function;
    auto callback = [](void* argument) {
        auto* entry = static_cast<Plain*>(argument);
        auto function = entry->function;
        free(entry);
        function();
    };
    if (__cxa_atexit(callback, entry, nullptr) != 0) {
        free(entry);
        return -1;
    }
    return 0;
}

// Optional completion hook called by the native CRT after main and before TLS
// destructors. This returns; it does not take ownership of Rust termination.
extern "C" void __scarlet_c_finish() {
    __cxa_finalize(nullptr);
    fflush(nullptr);
}

extern "C" [[noreturn]] void exit(int status) {
    __scarlet_c_finish();
    _Exit(status);
}
