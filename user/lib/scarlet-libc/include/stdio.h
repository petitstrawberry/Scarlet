#ifndef SCARLET_STDIO_H
#define SCARLET_STDIO_H
#include <stddef.h>
#include <stdarg.h>
#include <sys/types.h>
#ifdef __cplusplus
extern "C" {
#endif
/* Opaque, internally synchronized streams. This first implementation performs
   unbuffered I/O. Float, wide, positional and %n formatting are unsupported. */
typedef struct scarlet_FILE FILE;
#define EOF (-1)
#define SEEK_SET 0
#define SEEK_CUR 1
#define SEEK_END 2
#define BUFSIZ 8192
#define FILENAME_MAX 4096
extern FILE *stdin;
extern FILE *stdout;
extern FILE *stderr;
FILE *fopen(const char *, const char *);
FILE *fdopen(int, const char *);
int fclose(FILE *);
int fflush(FILE *);
size_t fread(void *, size_t, size_t, FILE *);
size_t fwrite(const void *, size_t, size_t, FILE *);
int fseek(FILE *, long, int);
long ftell(FILE *);
int fseeko(FILE *, off_t, int);
off_t ftello(FILE *);
void rewind(FILE *);
int feof(FILE *);
int ferror(FILE *);
void clearerr(FILE *);
int fileno(FILE *);
int fgetc(FILE *);
int getc(FILE *);
int getchar(void);
int fputc(int, FILE *);
int putc(int, FILE *);
int putchar(int);
int ungetc(int, FILE *);
char *fgets(char *, int, FILE *);
int fputs(const char *, FILE *);
int puts(const char *);
int snprintf(char *, size_t, const char *, ...);
int vsnprintf(char *, size_t, const char *, va_list);
int sprintf(char *, const char *, ...);
int vsprintf(char *, const char *, va_list);
int fprintf(FILE *, const char *, ...);
int vfprintf(FILE *, const char *, va_list);
int printf(const char *, ...);
int vprintf(const char *, va_list);
#ifdef __cplusplus
}
#endif
#endif
