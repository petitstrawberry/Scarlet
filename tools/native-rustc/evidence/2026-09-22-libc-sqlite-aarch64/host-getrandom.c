/* Host-only macOS compatibility for testing the same VFS: macOS exposes
 * getentropy(), not getrandom(). Scarlet uses its real getrandom export. */
#include <sys/types.h>
#include <sys/random.h>
#include <stddef.h>
#include <errno.h>
ssize_t getrandom(void *buffer, size_t size, unsigned int flags) {
    size_t done = 0;
    if (flags != 0) { errno = EINVAL; return -1; }
    while (done < size) {
        size_t n = size - done;
        if (n > 256) n = 256;
        if (getentropy((char *)buffer + done, n) < 0) return done ? (ssize_t)done : -1;
        done += n;
    }
    return (ssize_t)done;
}
