#ifndef SCARLET_CTYPE_H
#define SCARLET_CTYPE_H
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
#ifdef __cplusplus
}
#endif
#endif
