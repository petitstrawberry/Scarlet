#ifndef SCARLET_WEBCORE_MEMORY_H
#define SCARLET_WEBCORE_MEMORY_H

#include <stdbool.h>
#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif

/*
 * A handle is an opaque owning pointer allocated by this library, not a libc
 * descriptor or raw kernel handle. Pass only a live pointer returned by create
 * or duplicate. Synchronize calls with close and do not free the pointer.
 * Failures do not define an errno value.
 */

/* Returns a read/write shared-memory handle, or NULL on failure. Size must be
 * nonzero and no greater than the target's maximum signed pointer offset. */
void* scarlet_webcore_memory_create(size_t size);

/* Returns a separately owned handle to the same object, or NULL on failure.
 * The original handle remains owned by the caller. NULL returns NULL. */
void* scarlet_webcore_memory_duplicate(const void* handle);

/* Consumes the handle exactly once, including on failure. Returns 0 on success
 * and -1 on failure. NULL is accepted and returns 0. Existing mappings retain
 * their own ownership and must still be unmapped separately. */
int scarlet_webcore_memory_close(void* handle);

/* Maps size bytes at offset zero with shared read access and optional write
 * access. Retains the handle. Returns NULL on failure, including a NULL handle,
 * zero size, or a size above the maximum signed pointer offset. The caller owns
 * the entire returned mapping; mapping and handle lifetimes are independent. */
void* scarlet_webcore_memory_map(const void* handle, size_t size, bool writable);

/* Releases an entire owned mapping after all uses have ended. Supply its
 * original address and size. Returns 0 on success and -1 on failure, including
 * a NULL address or zero size. */
int scarlet_webcore_memory_unmap(void* address, size_t size);

#ifdef __cplusplus
}
#endif

#endif
