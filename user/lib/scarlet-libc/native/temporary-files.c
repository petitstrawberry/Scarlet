#include <errno.h>
#include <fcntl.h>
#include <stdlib.h>
#include <string.h>
#include <sys/random.h>
#include <sys/stat.h>
#include <unistd.h>

static const char alphabet[] = "abcdefghijklmnopqrstuvwxyz0123456789";

static int validate_template(char* text, size_t* length) {
    if (!text) { errno = EINVAL; return -1; }
    *length = strlen(text);
    if (*length < 6 || strcmp(text + *length - 6, "XXXXXX")) {
        errno = EINVAL;
        return -1;
    }
    return 0;
}

static int random_suffix(char* suffix) {
    unsigned char random[6];
    if (getentropy(random, sizeof(random))) return -1;
    for (unsigned i = 0; i < sizeof(random); ++i)
        suffix[i] = alphabet[random[i] % 36];
    return 0;
}

char* mkdtemp(char* text) {
    size_t length;
    if (validate_template(text, &length)) return NULL;
    for (int attempt = 0; attempt < 100; ++attempt) {
        if (random_suffix(text + length - 6)) return NULL;
        if (!mkdir(text, 0700)) return text;
        if (errno != EEXIST) return NULL;
    }
    errno = EEXIST;
    return NULL;
}

int mkostemp(char* text, int flags) {
    size_t length;
    if ((flags & ~O_CLOEXEC) || validate_template(text, &length)) {
        errno = EINVAL;
        return -1;
    }
    for (int attempt = 0; attempt < 100; ++attempt) {
        if (random_suffix(text + length - 6)) return -1;
        int fd = open(text, O_CREAT | O_EXCL | O_RDWR | flags, 0600);
        if (fd >= 0) return fd;
        if (errno != EEXIST) return -1;
    }
    errno = EEXIST;
    return -1;
}

int mkstemp(char* text) { return mkostemp(text, 0); }
