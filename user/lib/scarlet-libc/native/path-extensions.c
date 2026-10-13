#include <errno.h>
#include <fcntl.h>
#include <string.h>
#include <unistd.h>

int strerror_r(int error, char* buffer, size_t size) {
    const char* message = strerror(error);
    size_t length = strlen(message);
    if (!size || length >= size) return ERANGE;
    memcpy(buffer, message, length + 1);
    return 0;
}

int unlinkat(int directory, const char* path, int flags) {
    if (!path || (flags != 0 && flags != AT_REMOVEDIR)) {
        errno = EINVAL;
        return -1;
    }
    if (directory != AT_FDCWD && path[0] != '/') {
        errno = ENOTSUP;
        return -1;
    }
    return flags == AT_REMOVEDIR ? rmdir(path) : unlink(path);
}
