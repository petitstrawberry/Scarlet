#ifndef SCARLET_CTYPE_H
#define SCARLET_CTYPE_H
#include <locale.h>
#ifdef __cplusplus
extern "C" {
#endif
/* ASCII C locale. Arguments are EOF or a value representable as unsigned char. */
int isalnum(int);
int isalpha(int);
int isblank(int);
int iscntrl(int);
int isdigit(int);
int isgraph(int);
int islower(int);
int isprint(int);
int ispunct(int);
int isspace(int);
int isupper(int);
int isxdigit(int);
int tolower(int);
int toupper(int);
#define SCARLET_CTYPE_L(name) int name##_l(int, locale_t)
SCARLET_CTYPE_L(isalnum); SCARLET_CTYPE_L(isalpha); SCARLET_CTYPE_L(isblank);
SCARLET_CTYPE_L(iscntrl); SCARLET_CTYPE_L(isdigit); SCARLET_CTYPE_L(isgraph);
SCARLET_CTYPE_L(islower); SCARLET_CTYPE_L(isprint); SCARLET_CTYPE_L(ispunct);
SCARLET_CTYPE_L(isspace); SCARLET_CTYPE_L(isupper); SCARLET_CTYPE_L(isxdigit);
SCARLET_CTYPE_L(tolower); SCARLET_CTYPE_L(toupper);
#undef SCARLET_CTYPE_L
#ifdef __cplusplus
}
#endif
#endif
