#include <string.h>
#include <ctype.h>
#include <stdlib.h>
#include <errno.h>
#include <limits.h>

#define CHECK(expr) do { if (!(expr)) return __LINE__; } while (0)

/* Built with -fno-builtin to exercise the linked C ABI, including the memory
 * symbols supplied by the matching Rust compiler-builtins runtime. */
int scarlet_libc_strings_test(void) {
    unsigned char source[130], copied[134];
    for (size_t i = 0; i < sizeof(source); i++) source[i] = (unsigned char)i;
    for (size_t n = 0; n <= 129; n++) {
        CHECK(memset(copied, 0xaa, sizeof(copied)) == copied);
        CHECK(memcpy(copied + 1, source + 1, n) == copied + 1);
        CHECK(copied[0] == 0xaa && copied[n + 1] == 0xaa);
        CHECK(memcmp(copied + 1, source + 1, n) == 0);
        for (size_t i = 0; i < n; i++) CHECK(copied[i + 1] == source[i + 1]);
    }
    char overlap[12] = "abcdefghi";
    CHECK(memmove(overlap + 2, overlap, 7) == overlap + 2);
    CHECK(memcmp(overlap, "ababcdefg", 9) == 0);
    CHECK(memmove(overlap, overlap + 2, 7) == overlap);
    CHECK(memcmp(overlap, "abcdefg", 7) == 0);
    CHECK(memmove(overlap, overlap, 7) == overlap);
    const unsigned char high[] = {0xff, 0}, low[] = {0x7f, 0};
    CHECK(memcmp(high, low, 1) > 0 && strcmp((const char *)high, (const char *)low) > 0);
    CHECK(memchr(high, -1, 2) == high && memchr(high, 256, 2) == high + 1);
    CHECK(memchr(high, 7, 2) == NULL && memchr(high, 0xff, 0) == NULL);

    const char *text = "abaca";
    CHECK(strlen(text) == 5 && strlen("") == 0);
    CHECK(strnlen(text, 3) == 3 && strnlen(text, 30) == 5 && strnlen(text, 0) == 0);
    const char bounded[] = {'a', 'b', 'c'};
    CHECK(strnlen(bounded, sizeof(bounded)) == 3);
    CHECK(strncmp(bounded, "abd", 2) == 0 && strncmp(bounded, "abd", 3) < 0);
    CHECK(strncmp(bounded, "xyz", 0) == 0);
    CHECK(strchr(text, 'a') == text && strrchr(text, 'a') == text + 4);
    CHECK(strchr(text, 0) == text + 5 && strrchr(text, 0) == text + 5);
    CHECK(strchr(text, 256) == text + 5 && strrchr(text, 'z') == NULL);
    CHECK(strstr(text, "aca") == text + 2 && strstr(text, "") == text);
    CHECK(strstr(text, "abacaa") == NULL && strstr("", "a") == NULL);
    const char *repeated = "abababc";
    CHECK(strstr(repeated, "ababc") == repeated + 2);
    char buffer[16];
    memset(buffer, 'Q', sizeof(buffer));
    CHECK(strcpy(buffer, "a") == buffer);
    CHECK(strcat(buffer, "bc") == buffer);
    CHECK(strncat(buffer, bounded, 2) == buffer);
    CHECK(strcmp(buffer, "abcab") == 0 && buffer[6] == 'Q');
    CHECK(strncat(buffer, bounded, 0) == buffer && strcmp(buffer, "abcab") == 0);
    memset(buffer, 'Q', sizeof(buffer));
    CHECK(strncpy(buffer, "a", 4) == buffer);
    CHECK(memcmp(buffer, "a\0\0\0Q", 5) == 0);
    CHECK(strncpy(buffer, bounded, 3) == buffer);
    CHECK(memcmp(buffer, "abc\0Q", 5) == 0);
    errno = 73;
    char *owned = strdup(text);
    CHECK(owned != NULL && owned != text && strcmp(owned, text) == 0);
    owned[0] = 'z';
    CHECK(text[0] == 'a');
    free(owned);
    owned = strndup(bounded, 3);
    CHECK(owned != NULL && strcmp(owned, "abc") == 0);
    free(owned);
    owned = strndup(text, 0);
    CHECK(owned != NULL && owned[0] == 0);
    free(owned);
    CHECK(errno == 73);
    CHECK(strcmp(strerror(EINVAL), "Invalid argument") == 0);
    const char *saved_error = strerror(EINTR);
    CHECK(strcmp(strerror(-1), "Unknown error") == 0);
    CHECK(strcmp(saved_error, "Interrupted system call") == 0 && errno == 73);

    for (int c = -1; c <= 255; c++) {
        int alpha = (c >= 'A' && c <= 'Z') || (c >= 'a' && c <= 'z');
        int digit = c >= '0' && c <= '9';
        CHECK(!!isalpha(c) == alpha && !!isdigit(c) == digit);
        CHECK(!!isalnum(c) == (alpha || digit));
        CHECK(!!isblank(c) == (c == ' ' || c == '\t'));
        CHECK(!!iscntrl(c) == ((c >= 0 && c < 32) || c == 127));
        CHECK(!!isgraph(c) == (c > 32 && c < 127));
        CHECK(!!isprint(c) == (c >= 32 && c < 127));
        CHECK(!!ispunct(c) == (c > 32 && c < 127 && !alpha && !digit));
        CHECK(!!isspace(c) == (c == ' ' || (c >= '\t' && c <= '\r')));
        CHECK(!!isupper(c) == (c >= 'A' && c <= 'Z'));
        CHECK(!!islower(c) == (c >= 'a' && c <= 'z'));
        CHECK(!!isxdigit(c) == (digit || (c >= 'A' && c <= 'F') || (c >= 'a' && c <= 'f')));
        CHECK(tolower(c) == (c >= 'A' && c <= 'Z' ? c + 32 : c));
        CHECK(toupper(c) == (c >= 'a' && c <= 'z' ? c - 32 : c));
    }

    char *end;
    const char *number = " \t\n\v\f\r-0X7f!";
    errno = 73;
    CHECK(strtol(number, &end, 0) == -127 && *end == '!' && errno == 73);
    number = "+0759";
    CHECK(strtoul(number, &end, 0) == 61 && end == number + 4);
    number = "0x";
    CHECK(strtol(number, &end, 0) == 0 && end == number + 1);
    number = " +!";
    CHECK(strtol(number, &end, 10) == 0 && end == number && errno == 73);
    number = "0b101";
    CHECK(strtol(number, &end, 0) == 0 && end == number + 1);
    number = "123";
    CHECK(strtol(number, &end, 1) == 0 && end == number && errno == EINVAL);
    CHECK(strtol("9", &end, 37) == 0 && errno == EINVAL);
    errno = 73;
    CHECK(strtoll("9223372036854775807", &end, 10) == LLONG_MAX && *end == 0 && errno == 73);
    CHECK(strtoll("-9223372036854775808", &end, 10) == LLONG_MIN && *end == 0 && errno == 73);
    CHECK(strtoll("922337203685477580800!", &end, 10) == LLONG_MAX && *end == '!' && errno == ERANGE);
    errno = 73;
    CHECK(strtoll("-922337203685477580900!", &end, 10) == LLONG_MIN && *end == '!' && errno == ERANGE);
    errno = 73;
    CHECK(strtoull("18446744073709551615", &end, 10) == ULLONG_MAX && *end == 0 && errno == 73);
    CHECK(strtoull("-1", &end, 10) == ULLONG_MAX && *end == 0 && errno == 73);
    CHECK(strtoull("184467440737095516160!", &end, 10) == ULLONG_MAX && *end == '!' && errno == ERANGE);
    errno = 73;
    CHECK(strtoull("-18446744073709551616!", &end, 10) == ULLONG_MAX && *end == '!' && errno == ERANGE);
    errno = 73;
    CHECK(strtol("9223372036854775807", &end, 10) == LONG_MAX && *end == 0 && errno == 73);
    CHECK(strtoul("18446744073709551615", &end, 10) == ULONG_MAX && *end == 0 && errno == 73);
    CHECK(atoi(" -42!") == -42 && atol("+123!") == 123 && atoll("9223372036854775807") == LLONG_MAX);
    for (int base = 2; base <= 36; base++) {
        CHECK(strtoll("10!", &end, base) == base && *end == '!');
    }
    CHECK(errno == 73);
    return 0;
}
