#ifndef SCARLET_LOCALE_H
#define SCARLET_LOCALE_H
#include <stddef.h>
#define LC_CTYPE 0
#define LC_NUMERIC 1
#define LC_TIME 2
#define LC_COLLATE 3
#define LC_MONETARY 4
#define LC_MESSAGES 5
#define LC_ALL 6
#define LC_CTYPE_MASK (1 << LC_CTYPE)
#define LC_NUMERIC_MASK (1 << LC_NUMERIC)
#define LC_TIME_MASK (1 << LC_TIME)
#define LC_COLLATE_MASK (1 << LC_COLLATE)
#define LC_MONETARY_MASK (1 << LC_MONETARY)
#define LC_MESSAGES_MASK (1 << LC_MESSAGES)
#define LC_ALL_MASK 63
typedef struct scarlet_locale *locale_t;
#define LC_GLOBAL_LOCALE ((locale_t)-1)
struct lconv {
    char *decimal_point, *thousands_sep, *grouping;
    char *int_curr_symbol, *currency_symbol, *mon_decimal_point;
    char *mon_thousands_sep, *mon_grouping, *positive_sign, *negative_sign;
    char int_frac_digits, frac_digits, p_cs_precedes, p_sep_by_space;
    char n_cs_precedes, n_sep_by_space, p_sign_posn, n_sign_posn;
    char int_p_cs_precedes, int_p_sep_by_space, int_n_cs_precedes;
    char int_n_sep_by_space, int_p_sign_posn, int_n_sign_posn;
};
#ifdef __cplusplus
extern "C" {
#endif
/* This prototype implements C/POSIX only; other locales fail with ENOENT. */
char *setlocale(int, const char *);
struct lconv *localeconv(void);
locale_t newlocale(int, const char *, locale_t);
locale_t duplocale(locale_t);
void freelocale(locale_t);
locale_t uselocale(locale_t);
float strtof_l(const char *, char **, locale_t);
double strtod_l(const char *, char **, locale_t);
long double strtold_l(const char *, char **, locale_t);
long long strtoll_l(const char *, char **, int, locale_t);
unsigned long long strtoull_l(const char *, char **, int, locale_t);
#ifdef __cplusplus
}
#endif
#endif
