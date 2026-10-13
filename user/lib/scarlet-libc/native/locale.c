#include <errno.h>
#include <locale.h>
#include <string.h>

char* setlocale(int category, const char* name) {
    static char c_locale[] = "C";
    if (category < LC_CTYPE || category > LC_ALL) {
        errno = EINVAL;
        return NULL;
    }
    if (!name || !strcmp(name, "C") || !strcmp(name, "POSIX")) return c_locale;
    // Native locale support currently comprises C/POSIX only.
    errno = ENOENT;
    return NULL;
}
