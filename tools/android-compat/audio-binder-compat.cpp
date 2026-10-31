// Absorb the stock audio HAL's vndbinder shrink request. The vndbinder pool is
// intentionally started before the NDK pool request in the pinned service.
#include <dlfcn.h>
#include <stddef.h>

extern "C" int scarlet_set_thread_pool_max(void*, size_t)
    __asm__("_ZN7android12ProcessState27setThreadPoolMaxThreadCountEm");

extern "C" int scarlet_set_thread_pool_max(void* self, size_t count) {
    if (count <= 1) return 0;
    using Original = int (*)(void*, size_t);
    static Original original = reinterpret_cast<Original>(
        dlsym(RTLD_NEXT, "_ZN7android12ProcessState27setThreadPoolMaxThreadCountEm"));
    return original == nullptr ? -1 : original(self, count);
}

