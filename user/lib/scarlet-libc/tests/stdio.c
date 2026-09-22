#include <stdio.h>
#include <stdarg.h>
#include <stdint.h>
#include <limits.h>
#include <errno.h>
#include <fcntl.h>
#include <unistd.h>
#include <string.h>
#define CHECK(expr) do { if (!(expr)) return __LINE__; } while (0)

static int via_va(char *output, size_t size, const char *format, ...) {
    va_list args;
    va_start(args, format);
    int result = vsnprintf(output, size, format, args);
    va_end(args);
    return result;
}
static int file_via_va(FILE *file, const char *format, ...) {
    va_list args;
    va_start(args, format);
    int result = vfprintf(file, format, args);
    va_end(args);
    return result;
}

/* Every run owns a new path under directory. Return the failing source line. */
int scarlet_libc_stdio_test(const char *directory) {
    char text[256], path[FILENAME_MAX];
    CHECK(snprintf(path, sizeof(path), "%s/c-stdio.txt", directory) > 0);
    CHECK(snprintf(NULL, 0, "%d:%s", -12, "hello") == 9);
    char tiny[5] = {1, 2, 3, 4, 5};
    CHECK(snprintf(tiny, sizeof(tiny), "%s", "abcdefgh") == 8);
    CHECK(strcmp(tiny, "abcd") == 0);
    CHECK(snprintf(tiny, 1, "abc") == 3 && tiny[0] == 0);
    tiny[0] = 42;
    CHECK(snprintf(tiny, 0, "abc") == 3 && tiny[0] == 42);
    CHECK(via_va(text, sizeof(text), "%d/%u/%ld/%llu/%zu/%td/%hhd/%hhu/%x", -1, 2u,
        -3L, 4ULL, (size_t)5, (ptrdiff_t)-6, -7, 255, 0xabu) == 24);
    CHECK(strcmp(text, "-1/2/-3/4/5/-6/-7/255/ab") == 0);
    /* Cross register-save area and spill into stack arguments on both ABIs. */
    CHECK(via_va(text, sizeof(text), "%d%d%d%d%d%d%d%d%d%d:%s", 0, 1, 2, 3, 4, 5,
        6, 7, 8, 9, "stack") == 16);
    CHECK(strcmp(text, "0123456789:stack") == 0);
    CHECK(snprintf(text, sizeof(text), "%#08x|%-6.3s|%+06d|%.*u", 42u, "abcdef", -12, 0, 0u) == 23);
    CHECK(strcmp(text, "0x00002a|abc   |-00012|") == 0);
    CHECK(snprintf(text, sizeof(text), "%lld", (-9223372036854775807LL - 1)) == 20);
    CHECK(strcmp(text, "-9223372036854775808") == 0);
    /* Unsupported formats fail explicitly, rather than emit an invented value. */
    CHECK(snprintf(text, sizeof(text), "%f", 1.0) == -1 && errno == ENOTSUP);
    CHECK(snprintf(NULL, 0, "%2147483647dX", 0) == -1 && errno == EOVERFLOW);

    FILE *file = fopen(path, "w+x");
    CHECK(file != NULL);
    CHECK(fopen(path, "wx") == NULL && errno == EEXIST);
    CHECK(fprintf(file, "%s:%04d\n", "first", 17) == 11);
    CHECK(file_via_va(file, "%s %lld\n", "second", -1234567890123LL) == 22);
    CHECK(fwrite("ABCDE", 1, 5, file) == 5);
    CHECK(fflush(file) == 0 && fflush(NULL) == 0);
    CHECK(ftell(file) == 38);
    CHECK(fseek(file, 0, SEEK_SET) == 0);
    CHECK(fgets(text, sizeof(text), file) == text && strcmp(text, "first:0017\n") == 0);
    CHECK(fgetc(file) == 's');
    CHECK(ungetc('X', file) == 'X');
    CHECK(ftell(file) == 11);
    CHECK(fgetc(file) == 'X');
    CHECK(fgets(text, sizeof(text), file) == text && strcmp(text, "econd -1234567890123\n") == 0);
    CHECK(fread(text, 2, 3, file) == 2); /* Partial last item consumes its byte. */
    CHECK(memcmp(text, "ABCDE", 5) == 0 && feof(file) && !ferror(file));
    CHECK(fgetc(file) == EOF);
    CHECK(ungetc('Z', file) == 'Z' && !feof(file));
    CHECK(fgetc(file) == 'Z');
    CHECK(fgetc(file) == EOF && feof(file));
    clearerr(file);
    CHECK(!feof(file) && !ferror(file));
    rewind(file);
    CHECK(ftell(file) == 0 && !feof(file));
    CHECK(fgetc(file) == 'f' && ungetc('Y', file) == 'Y');
    CHECK(fseek(file, 0, SEEK_CUR) == 0 && fgetc(file) == 'f');
    CHECK(ungetc('Y', file) == 'Y');
    CHECK(fflush(file) == 0 && lseek(fileno(file), 0, SEEK_CUR) == 0);
    CHECK(fgetc(file) == 'f');
    CHECK(fwrite(text, (size_t)-1, 2, file) == 0 && errno == EOVERFLOW && ferror(file));
    clearerr(file);
    CHECK(fread(NULL, 0, 99, file) == 0 && !ferror(file));
    CHECK(fclose(file) == 0);

    file = fopen(path, "r");
    CHECK(file != NULL);
    CHECK(fputc('!', file) == EOF && errno == EBADF && ferror(file));
    clearerr(file);
    CHECK(fputs("", file) == EOF && errno == EBADF && ferror(file));
    CHECK(fclose(file) == 0);
    file = fopen(path, "a+");
    CHECK(file != NULL);
    CHECK(fseek(file, 0, SEEK_SET) == 0);
    CHECK(fputs("TAIL", file) >= 0);
    CHECK(fseek(file, -4, SEEK_END) == 0);
    CHECK(fread(text, 1, 4, file) == 4 && memcmp(text, "TAIL", 4) == 0);
    CHECK(fclose(file) == 0);

    int fd = open(path, O_RDONLY);
    CHECK(fd >= 0);
    CHECK(fdopen(fd, "w") == NULL && errno == EINVAL);
    CHECK(lseek(fd, 0, SEEK_CUR) == 0); /* Failed fdopen retains ownership. */
    file = fdopen(fd, "r");
    CHECK(file != NULL && fileno(file) == fd);
    CHECK(fgetc(file) == 'f');
    CHECK(fclose(file) == 0);
    CHECK(read(fd, text, 1) == -1 && errno == EBADF);
    fd = open(path, O_WRONLY);
    CHECK(fd >= 0);
    file = fdopen(fd, "a");
    CHECK(file != NULL);
    CHECK(fseek(file, 0, SEEK_SET) == 0 && fputc('!', file) == '!');
    CHECK(fclose(file) == 0);
    file = fopen(path, "r");
    CHECK(file != NULL);
    CHECK(fseek(file, -1, SEEK_END) == 0 && fgetc(file) == '!');
    CHECK(fclose(file) == 0);
    CHECK(fopen(path, "r++") == NULL && errno == EINVAL);
    CHECK(fdopen(-1, "r") == NULL && errno == EBADF);
    return 0;
}
