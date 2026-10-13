// Native Scarlet anonymous temporary stream; names come from real getentropy
// via mkostemp, and O_EXCL prevents replacement of an existing file.
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <unistd.h>
FILE *tmpfile(void) {
    char path[] = "/tmp/scarlet-stdio-XXXXXX";
    int fd = mkostemp(path, O_CLOEXEC);
    if (fd < 0) return NULL;
    if (unlink(path) != 0) {
        int error = errno;
        close(fd);
        errno = error;
        return NULL;
    }
    FILE *stream = fdopen(fd, "w+b");
    if (!stream) {
        int error = errno;
        close(fd);
        errno = error;
    }
    return stream;
}
