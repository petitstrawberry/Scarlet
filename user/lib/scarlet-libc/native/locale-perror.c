/* Native Scarlet C/POSIX-only locale and stream-based errno reporting. */
#include <errno.h>
#include <limits.h>
#include <locale.h>
#include <stdio.h>
#include <string.h>

struct lconv *localeconv(void) {
    static char point[] = ".";
    static char empty[] = "";
    static struct lconv c_locale = {
        .decimal_point = point,
        .thousands_sep = empty,
        .grouping = empty,
        .int_curr_symbol = empty,
        .currency_symbol = empty,
        .mon_decimal_point = empty,
        .mon_thousands_sep = empty,
        .mon_grouping = empty,
        .positive_sign = empty,
        .negative_sign = empty,
        .int_frac_digits = CHAR_MAX,
        .frac_digits = CHAR_MAX,
        .p_cs_precedes = CHAR_MAX,
        .p_sep_by_space = CHAR_MAX,
        .n_cs_precedes = CHAR_MAX,
        .n_sep_by_space = CHAR_MAX,
        .p_sign_posn = CHAR_MAX,
        .n_sign_posn = CHAR_MAX,
        .int_p_cs_precedes = CHAR_MAX,
        .int_p_sep_by_space = CHAR_MAX,
        .int_n_cs_precedes = CHAR_MAX,
        .int_n_sep_by_space = CHAR_MAX,
        .int_p_sign_posn = CHAR_MAX,
        .int_n_sign_posn = CHAR_MAX,
    };
    return &c_locale;
}

void perror(const char *prefix) {
    int saved_errno = errno;
    const char *message = strerror(saved_errno);
    if (prefix && *prefix) {
        fputs(prefix, stderr);
        fputs(": ", stderr);
    }
    fputs(message, stderr);
    fputc('\n', stderr);
    errno = saved_errno;
}
