#ifndef SCARLET_SYS_RANDOM_H
#define SCARLET_SYS_RANDOM_H
#include <sys/types.h>
#define GRND_NONBLOCK 1U
#define GRND_RANDOM 2U
#ifdef __cplusplus
extern "C" {
#endif
/* flags=0 requires a registered entropy source; the emergency PRNG is never
 * used. Recognized flags above return ENOTSUP. Native errors (including no
 * entropy source) currently return EIO because the syscall has one sentinel.
 * Calls may block. A failing call may have changed part of the output buffer. */
ssize_t getrandom(void *, size_t, unsigned int);
/* At most 256 bytes; a successful call fills the entire buffer. */
int getentropy(void *, size_t);
#ifdef __cplusplus
}
#endif
#endif
