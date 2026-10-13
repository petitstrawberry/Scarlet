// Deterministic, concurrency-safe, non-cryptographic PRNG for the C API.
// Unsigned arithmetic defines a full-period 32-bit LCG. The high 31 bits
// satisfy 0 <= rand() <= RAND_MAX. Default initialization equals srand(1).
#include <stdint.h>
#include <stdlib.h>
#include <stdatomic.h>
_Static_assert(RAND_MAX == 2147483647, "Scarlet rand uses 31 output bits");
_Static_assert(sizeof(unsigned int) == 4, "Scarlet AArch64 unsigned ABI");
static _Atomic uint32_t state = 1;
void srand(unsigned int seed) {
    atomic_store_explicit(&state, seed, memory_order_relaxed);
}
int rand(void) {
    uint32_t old = atomic_load_explicit(&state, memory_order_relaxed), next;
    do { next = old * UINT32_C(1664525) + UINT32_C(1013904223); }
    while (!atomic_compare_exchange_weak_explicit(&state, &old, next,
                                                  memory_order_relaxed,
                                                  memory_order_relaxed));
    return (int)(next >> 1);
}
