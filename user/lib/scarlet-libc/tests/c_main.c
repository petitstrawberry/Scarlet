#include <errno.h>
#include <stdlib.h>

static int constructor_ran;
static int constructor_failed;
static int *constructor_errno;

/* The standard Scarlet CRT must initialize the native TLS header before
   dispatching this C constructor; no Rust executable source is involved. */
__attribute__((constructor)) static void before_main(void) {
    constructor_errno = &errno;
    if (errno != 0) {
        constructor_failed = 1;
        return;
    }
    errno = ERANGE;
    unsigned char *p = malloc(3);
    if (p == NULL || (size_t)p % 16 != 0 || errno != ERANGE) {
        constructor_failed = 2;
        return;
    }
    p[0] = 17;
    p[2] = 29;
    free(p);
    if (errno != ERANGE) {
        constructor_failed = 3;
        return;
    }
    errno = 0;
    constructor_ran = 1;
}

#define CHECK(expression) do { if (!(expression)) return __LINE__ + 50; } while (0)

int main(int argc, char **argv) {
    CHECK(constructor_failed == 0 && constructor_ran == 1);
    CHECK(&errno == constructor_errno && errno == 0);
    CHECK(argc >= 1 && argv != NULL && argv[0] != NULL && argv[0][0] != '\0');
    CHECK(argv[argc] == NULL);

    errno = ERANGE;
    unsigned char *p = malloc(7);
    CHECK(p != NULL && (size_t)p % 16 == 0 && errno == ERANGE);
    for (size_t i = 0; i < 7; i++) p[i] = (unsigned char)(31 + i);
    unsigned char *grown = reallocarray(p, 13, 3);
    CHECK(grown != NULL && errno == ERANGE);
    for (size_t i = 0; i < 7; i++) CHECK(grown[i] == (unsigned char)(31 + i));
    CHECK(reallocarray(grown, (size_t)-1, 2) == NULL && errno == ENOMEM);
    for (size_t i = 0; i < 7; i++) CHECK(grown[i] == (unsigned char)(31 + i));
    free(grown);
    CHECK(errno == ENOMEM);

    errno = ERANGE;
    p = aligned_alloc(4096, 65);
    CHECK(p != NULL && (size_t)p % 4096 == 0 && errno == ERANGE);
    p[0] = 19;
    p[64] = 37;
    unsigned char *resized = realloc(p, 129);
    CHECK(resized != NULL && resized[0] == 19 && resized[64] == 37);
    free(resized);
    CHECK(errno == ERANGE);

    void *output = &constructor_ran;
    CHECK(posix_memalign(&output, 3, 65) == EINVAL);
    CHECK(output == &constructor_ran && errno == ERANGE);
    CHECK(posix_memalign(&output, 64, (size_t)-1) == ENOMEM);
    CHECK(output == &constructor_ran && errno == ERANGE);
    CHECK(posix_memalign(&output, 64, 65) == 0);
    CHECK(output != NULL && (size_t)output % 64 == 0 && errno == ERANGE);
    p = output;
    p[0] = 23;
    p[64] = 41;
    free(output);
    free(NULL);
    CHECK(errno == ERANGE);
    return 43;
}
