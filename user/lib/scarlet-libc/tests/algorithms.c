#include <stdlib.h>
#include <string.h>
#include <strings.h>
#include <stdint.h>
#include <limits.h>
#include <errno.h>

#define CHECK(expr) do { if (!(expr)) return __LINE__; } while (0)

/* Odd-size records start deliberately one byte off alignment. The comparator
 * verifies qsort/bsearch pass elements in the input, never a copied pivot. */
static unsigned char *array_start;
static size_t array_count;
static size_t comparisons;
static int bad_pointer;
static const void *search_key;

static unsigned key_of(const void *pointer) {
    const unsigned char *p = pointer;
    return (unsigned)p[0] | ((unsigned)p[1] << 8);
}

static int element_pointer(const void *pointer) {
    uintptr_t address = (uintptr_t)pointer, base = (uintptr_t)array_start;
    return address >= base && address - base < array_count * 3 && (address - base) % 3 == 0;
}

static int record_order(const void *a, const void *b) {
    comparisons++;
    if (!element_pointer(a) || !element_pointer(b)) bad_pointer = 1;
    unsigned left = key_of(a), right = key_of(b);
    return left < right ? INT_MIN : left > right ? INT_MAX : 0;
}

static int search_order(const void *a, const void *b) {
    comparisons++;
    if (a != search_key || !element_pointer(b)) bad_pointer = 1;
    unsigned left = key_of(a), right = key_of(b);
    return left < right ? INT_MIN : left > right ? INT_MAX : 0;
}

static void write_record(unsigned char *out, unsigned key) {
    out[0] = (unsigned char)key;
    out[1] = (unsigned char)(key >> 8);
    out[2] = (unsigned char)(out[0] ^ out[1] ^ 0xa5);
}

int scarlet_libc_algorithms_test(void) {
    unsigned char storage[4096 * 3 + 2];
    const size_t sizes[] = {0, 1, 2, 3, 4, 31, 32, 33, 4096};
    errno = 73;
    for (size_t s = 0; s < sizeof(sizes) / sizeof(sizes[0]); s++) {
        size_t n = sizes[s];
        for (unsigned shape = 0; shape < 5; shape++) {
            unsigned frequencies[257] = {0};
            memset(storage, 0x5a, sizeof(storage));
            array_start = storage + 1;
            array_count = n;
            for (size_t i = 0; i < n; i++) {
                unsigned key;
                if (shape == 0) key = (unsigned)(i % 257);
                else if (shape == 1) key = (unsigned)((n - i) % 257);
                else if (shape == 2) key = 7;
                else if (shape == 3) key = (unsigned)((i * 167 + 53) % 257);
                else key = (unsigned)((i < n / 2 ? i : n - i) % 257);
                write_record(array_start + i * 3, key);
                frequencies[key]++;
            }
            comparisons = 0;
            bad_pointer = 0;
            qsort(array_start, n, 3, record_order);
            CHECK(!bad_pointer && comparisons <= n * 64);
            CHECK(storage[0] == 0x5a && storage[n * 3 + 1] == 0x5a);
            unsigned previous = 0;
            for (size_t i = 0; i < n; i++) {
                unsigned char *record = array_start + i * 3;
                unsigned key = key_of(record);
                CHECK(key >= previous && key <= 256 && frequencies[key] > 0);
                CHECK(record[2] == (unsigned char)(record[0] ^ record[1] ^ 0xa5));
                frequencies[key]--;
                previous = key;
            }
            for (unsigned i = 0; i < 257; i++) CHECK(frequencies[i] == 0);
            const unsigned keys[] = {0, 1, 7, 128, 255, 256, 257, 65535};
            for (size_t i = 0; i < sizeof(keys) / sizeof(keys[0]); i++) {
                unsigned char key_record[3];
                write_record(key_record, keys[i]);
                search_key = key_record;
                comparisons = 0;
                void *found = bsearch(key_record, array_start, n, 3, search_order);
                int expected = 0;
                for (size_t j = 0; j < n; j++) {
                    if (key_of(array_start + j * 3) == keys[i]) expected = 1;
                }
                CHECK(!bad_pointer && comparisons <= 13);
                CHECK((found != NULL) == expected);
                if (found) CHECK(element_pointer(found) && key_of(found) == keys[i]);
            }
        }
    }
    comparisons = 0;
    qsort(storage, 0, 3, record_order);
    qsort(storage, 5, 0, record_order);
    CHECK(bsearch(storage, storage, 0, 3, search_order) == NULL);
    CHECK(comparisons == 0 && errno == 73);
    CHECK(abs(-INT_MAX) == INT_MAX && abs(0) == 0 && abs(1) == 1);
    CHECK(labs(-LONG_MAX) == LONG_MAX && llabs(-LLONG_MAX) == LLONG_MAX);

    const char *text = "abbcde";
    CHECK(strspn(text, "ab") == 3 && strspn(text, "") == 0);
    CHECK(strspn("", "a") == 0 && strspn(text, "edcba") == 6);
    CHECK(strcspn(text, "cb") == 1 && strcspn(text, "") == 6);
    CHECK(strcspn("", "a") == 0 && strcspn(text, "xyz") == 6);
    CHECK(strpbrk(text, "cba") == text && strpbrk(text, "cd") == text + 3);
    CHECK(strpbrk(text, "") == NULL && strpbrk(text, "xyz") == NULL);
    for (unsigned i = 1; i <= 255; i++) {
        char repeated[] = {(char)i, (char)i, 0};
        char set[] = {(char)i, 0};
        CHECK(strspn(repeated, set) == 2 && strcspn(repeated, set) == 0);
        CHECK(strpbrk(repeated, set) == repeated);
    }
    CHECK(strcasecmp("AbCd", "aBcD") == 0 && strcasecmp("a", "B") < 0);
    CHECK(strcasecmp("Ab", "a") > 0 && strcasecmp("", "") == 0);
    const char high[] = {(char)0xff, 0}, low[] = {(char)0x80, 0};
    CHECK(strcasecmp(high, low) > 0 && strcasecmp(low, "Z") > 0);
    const char bounded[] = {'A', 'b', 'C'};
    CHECK(strncasecmp(bounded, "abc", 3) == 0);
    CHECK(strncasecmp("abcX", "ABCy", 3) == 0 && strncasecmp("abcX", "ABCy", 4) < 0);
    CHECK(strncasecmp("", "anything", 0) == 0);

    char first[] = ",,one:two;three,,", second[] = "x/y";
    char *state = NULL, *other = NULL;
    CHECK(strcmp(strtok_r(first, ",:", &state), "one") == 0);
    CHECK(strcmp(strtok_r(second, "/", &other), "x") == 0);
    CHECK(strcmp(strtok_r(NULL, ";", &state), "two") == 0);
    CHECK(strcmp(strtok_r(NULL, ",", &state), "three") == 0);
    CHECK(strtok_r(NULL, ",", &state) == NULL);
    CHECK(strtok_r(NULL, ",", &state) == NULL);
    CHECK(strcmp(strtok_r(NULL, "", &other), "y") == 0);
    CHECK(strtok_r(NULL, "", &other) == NULL);
    char high_tokens[] = {(char)0xff, 'a', (char)0xff, 'b', 0};
    char high_delimiter[] = {(char)0xff, 0};
    CHECK(strcmp(strtok_r(high_tokens, high_delimiter, &state), "a") == 0);
    CHECK(strcmp(strtok_r(NULL, high_delimiter, &state), "b") == 0);
    CHECK(strtok_r(NULL, high_delimiter, &state) == NULL);
    char empty[] = "", all_delimiters[] = ",,,", no_delimiters[] = "a,b";
    CHECK(strtok_r(empty, ",", &state) == NULL);
    CHECK(strtok_r(all_delimiters, ",", &state) == NULL);
    CHECK(strtok_r(no_delimiters, "", &state) == no_delimiters);
    CHECK(strcmp(no_delimiters, "a,b") == 0 && strtok_r(NULL, "", &state) == NULL);
    char standard[] = "a b c";
    CHECK(strcmp(strtok(standard, " "), "a") == 0);
    CHECK(strcmp(strtok(NULL, " "), "b") == 0);
    CHECK(strcmp(strtok(NULL, " "), "c") == 0);
    CHECK(strtok(NULL, " ") == NULL && strtok(NULL, " ") == NULL);
    CHECK(errno == 73);
    return 0;
}
