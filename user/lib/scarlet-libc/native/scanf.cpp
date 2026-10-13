// String-only C-locale scanf for native Scarlet.
// Numeric conversion uses the separately pinned LLVM 21.1.8 header-only core.
// See README.md for the supported-format contract and explicit rejections.
#include <errno.h>
#include <limits.h>
#include <stddef.h>
#include <stdint.h>
#include <stdarg.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "shared/str_to_float.h"

namespace {
enum class Length { none, hh, h, l, ll, j, z, t, big_l };
struct Spec {
    bool suppress = false;
    size_t width = SIZE_MAX;
    Length length = Length::none;
    char conversion = 0;
    bool invert = false;
    bool set[256] = {};
};
bool space(unsigned char c) {
    return c == ' ' || c == '\t' || c == '\n' || c == '\r' || c == '\f' || c == '\v';
}
bool digit(unsigned char c) { return c >= '0' && c <= '9'; }
unsigned char lower(unsigned char c) { return c >= 'A' && c <= 'Z' ? c + ('a' - 'A') : c; }
int value(unsigned char c) {
    if (digit(c)) return c - '0';
    c = lower(c);
    return c >= 'a' && c <= 'f' ? c - 'a' + 10 : -1;
}
bool one_of(char c, const char *s) { return c && strchr(s, c); }

// Validate every directive before inspecting va_args or changing destinations.
// Reject extensions and wide text explicitly rather than silently misparsing.
bool parse_spec(const char *&f, Spec &s) {
    if (*f == '*') { s.suppress = true; ++f; }
    if (digit(*f)) {
        size_t width = 0;
        do {
            unsigned d = *f++ - '0';
            if (width > (SIZE_MAX - d) / 10) { errno = EINVAL; return false; }
            width = width * 10 + d;
        } while (digit(*f));
        if (!width) { errno = EINVAL; return false; }
        s.width = width;
    }
    if (*f == 'h') { ++f; s.length = Length::h; if (*f == 'h') { ++f; s.length = Length::hh; } }
    else if (*f == 'l') { ++f; s.length = Length::l; if (*f == 'l') { ++f; s.length = Length::ll; } }
    else if (*f == 'j') { ++f; s.length = Length::j; }
    else if (*f == 'z') { ++f; s.length = Length::z; }
    else if (*f == 't') { ++f; s.length = Length::t; }
    else if (*f == 'L') { ++f; s.length = Length::big_l; }
    s.conversion = *f;
    if (!*f) { errno = EINVAL; return false; }
    ++f;
    if (s.conversion == '[') {
        if (*f == '^') { s.invert = true; ++f; }
        bool any = false;
        if (*f == ']') { s.set[(unsigned char)*f++] = true; any = true; }
        while (*f && *f != ']') {
            unsigned char first = *f++;
            any = true;
            // C permits implementation-defined ranges; Scarlet uses ascending
            // unsigned-byte ranges. A descending range is three literal bytes.
            if (*f == '-' && f[1] && f[1] != ']' && first <= (unsigned char)f[1]) {
                unsigned char last = f[1]; f += 2;
                for (unsigned c = first; c <= last; ++c) s.set[c] = true;
            } else s.set[first] = true;
        }
        if (!any || *f != ']') { errno = EINVAL; return false; }
        ++f;
    }
    if (one_of(s.conversion, "diuoxXn")) {
        if (s.length == Length::big_l) { errno = EINVAL; return false; }
        if (s.conversion == 'n' && (s.suppress || s.width != SIZE_MAX)) { errno = EINVAL; return false; }
        return true;
    }
    if (one_of(s.conversion, "aAeEfFgG")) {
        if (s.length == Length::big_l) { errno = ENOTSUP; return false; }
        if (s.length != Length::none && s.length != Length::l) { errno = EINVAL; return false; }
        return true;
    }
    if (one_of(s.conversion, "cs[")) {
        if (s.length == Length::l) { errno = ENOTSUP; return false; }
        if (s.length != Length::none) { errno = EINVAL; return false; }
        return true;
    }
    if (s.conversion == 'p') {
        if (s.length != Length::none) { errno = EINVAL; return false; }
        return true;
    }
    if (s.conversion == '%') {
        if (s.length != Length::none || s.suppress || s.width != SIZE_MAX) { errno = EINVAL; return false; }
        return true;
    }
    errno = ENOTSUP; // Includes POSIX %m, positional $, and nonstandard forms.
    return false;
}
bool valid_format(const char *f) {
    while (*f) if (*f++ == '%') { Spec s; if (!parse_spec(f, s)) return false; }
    return true;
}

template<class T> bool put_signed(va_list &args, uintmax_t magnitude, bool negative) {
    uintmax_t maximum = (uintmax_t)__llvm_libc::cpp::numeric_limits<T>::max();
    if (magnitude > maximum + (negative ? 1 : 0)) { errno = ERANGE; return false; }
    T result = negative ? static_cast<T>(-static_cast<intmax_t>(magnitude - (magnitude != 0))) - (magnitude != 0)
                        : static_cast<T>(magnitude);
    *va_arg(args, T *) = result;
    return true;
}
template<class T> bool put_unsigned(va_list &args, uintmax_t magnitude, bool negative) {
    if (magnitude > (uintmax_t)__llvm_libc::cpp::numeric_limits<T>::max()) { errno = ERANGE; return false; }
    *va_arg(args, T *) = negative ? T(0) - static_cast<T>(magnitude) : static_cast<T>(magnitude);
    return true;
}
bool put_integer(va_list &args, const Spec &s, uintmax_t n, bool negative, bool is_signed) {
    if (s.suppress) return true;
#define WRITE_INTEGER(S, U) return is_signed ? put_signed<S>(args, n, negative) : put_unsigned<U>(args, n, negative)
    switch (s.length) {
    case Length::hh: WRITE_INTEGER(signed char, unsigned char);
    case Length::h: WRITE_INTEGER(short, unsigned short);
    case Length::none: WRITE_INTEGER(int, unsigned int);
    case Length::l: WRITE_INTEGER(long, unsigned long);
    case Length::ll: WRITE_INTEGER(long long, unsigned long long);
    case Length::j: WRITE_INTEGER(intmax_t, uintmax_t);
    // Scarlet AArch64 uses the LP64 ABI: size_t unsigned long, ptrdiff_t long.
    case Length::z: WRITE_INTEGER(long, size_t);
    case Length::t: WRITE_INTEGER(ptrdiff_t, unsigned long);
    case Length::big_l: errno = EINVAL; return false;
    }
#undef WRITE_INTEGER
    return false;
}

struct Number { size_t size; uintmax_t magnitude; bool negative; bool good; };
Number integer_token(const char *p, size_t width, unsigned base, bool suppress) {
    size_t n = 0; bool negative = false;
    if (n < width && (p[n] == '+' || p[n] == '-')) negative = p[n++] == '-';
    if (n < width && p[n] == '0') {
        if (n + 1 < width && lower(p[n + 1]) == 'x' && (base == 0 || base == 16)) {
            base = 16; n += 2;
        } else if (!base) base = 8;
    }
    if (!base) base = 10;
    size_t digits = 0; uintmax_t magnitude = 0; bool overflow = false;
    while (n < width && p[n]) {
        int d = value(p[n]);
        if (d < 0 || (unsigned)d >= base) break;
        if (magnitude > (UINTMAX_MAX - (unsigned)d) / base) overflow = true;
        else magnitude = magnitude * base + (unsigned)d;
        ++n; ++digits;
    }
    if (overflow && !suppress) errno = ERANGE;
    return {n, magnitude, negative, digits != 0 && (!overflow || suppress)};
}

struct FloatToken { size_t size; bool good; };
FloatToken float_token(const char *p, size_t width) {
    size_t n = 0;
    if (n < width && (p[n] == '+' || p[n] == '-')) ++n;
    if (n >= width || !p[n]) return {n, false};
    if (lower(p[n]) == 'i') {
        size_t matched = 0;
        const char *word = "infinity";
        while (n < width && word[matched] && lower(p[n]) == word[matched]) { ++matched; ++n; }
        return {n, matched == 3 || matched == 8};
    }
    if (lower(p[n]) == 'n') {
        size_t matched = 0;
        const char *word = "nan";
        while (n < width && word[matched] && lower(p[n]) == word[matched]) { ++matched; ++n; }
        if (matched != 3) return {n, false};
        if (n < width && p[n] == '(') {
            ++n;
            while (n < width && (digit(p[n]) || (lower(p[n]) >= 'a' && lower(p[n]) <= 'z') || p[n] == '_')) ++n;
            if (n >= width || p[n] != ')') return {n, false};
            ++n;
        }
        return {n, true};
    }
    bool hex = false;
    if (n < width && p[n] == '0' && n + 1 < width && lower(p[n + 1]) == 'x') { hex = true; n += 2; }
    unsigned base = hex ? 16 : 10;
    size_t digits = 0;
    while (n < width && value(p[n]) >= 0 && (unsigned)value(p[n]) < base) { ++n; ++digits; }
    if (n < width && p[n] == '.') {
        ++n;
        while (n < width && value(p[n]) >= 0 && (unsigned)value(p[n]) < base) { ++n; ++digits; }
    }
    if (!digits) return {n, false};
    if (n < width && lower(p[n]) == (hex ? 'p' : 'e')) {
        ++n;
        if (n < width && (p[n] == '+' || p[n] == '-')) ++n;
        size_t exponent_digits = 0;
        while (n < width && digit(p[n])) { ++n; ++exponent_digits; }
        if (!exponent_digits) return {n, false};
    }
    return {n, true};
}

bool put_float(va_list &args, const Spec &s, const char *input, size_t n) {
    if (s.suppress) return true;
    char local[256];
    if (n == SIZE_MAX) { errno = ENOMEM; return false; }
    char *text = n < sizeof(local) ? local : static_cast<char *>(malloc(n + 1));
    if (!text) { errno = ENOMEM; return false; }
    memcpy(text, input, n); text[n] = '\0';
    bool ok;
    if (s.length == Length::l) {
        auto result = __llvm_libc::internal::strtofloatingpoint<double>(text);
        ok = static_cast<size_t>(result.parsed_len) == n;
        if (ok) { *va_arg(args, double *) = result.value; if (result.has_error()) errno = result.error; }
    } else {
        auto result = __llvm_libc::internal::strtofloatingpoint<float>(text);
        ok = static_cast<size_t>(result.parsed_len) == n;
        if (ok) { *va_arg(args, float *) = result.value; if (result.has_error()) errno = result.error; }
    }
    if (text != local) free(text);
    if (!ok) errno = EINVAL;
    return ok;
}

int scan(const char *input, const char *format, va_list &args) {
    if (!input || !format) { errno = EINVAL; return EOF; }
    if (!valid_format(format)) return EOF;
    const char *p = input;
    int assignments = 0;
    bool completed = false;
    while (*format) {
        if (space(*format)) {
            do { ++format; } while (space(*format));
            while (space(*p)) ++p;
            continue;
        }
        if (*format != '%') {
            if (*p != *format) return !*p && !completed ? EOF : assignments;
            ++p; ++format; continue;
        }
        ++format;
        Spec s;
        if (!parse_spec(format, s)) return assignments; // Already validated.
        char c = s.conversion;
        if (c == '%') {
            while (space(*p)) ++p;
            if (*p != '%') return !*p && !completed ? EOF : assignments;
            ++p; continue;
        }
        if (c == 'n') {
            if (!put_integer(args, s, p - input, false, true)) return assignments;
            completed = true; continue;
        }
        if (c != 'c' && c != '[') while (space(*p)) ++p;
        if (!*p) return completed ? assignments : EOF;
        if (one_of(c, "diuoxXp")) {
            unsigned base = c == 'i' ? 0 : c == 'o' ? 8 : one_of(c, "xXp") ? 16 : 10;
            Number n = integer_token(p, s.width, base, s.suppress);
            if (!n.good) return assignments;
            if (c == 'p') {
                if (!s.suppress) *va_arg(args, void **) = reinterpret_cast<void *>(static_cast<uintptr_t>(n.negative ? uintmax_t(0) - n.magnitude : n.magnitude));
            } else if (!put_integer(args, s, n.magnitude, n.negative, c == 'd' || c == 'i')) return assignments;
            p += n.size;
        } else if (one_of(c, "aAeEfFgG")) {
            FloatToken n = float_token(p, s.width);
            if (!n.good || !put_float(args, s, p, n.size)) return assignments;
            p += n.size;
        } else {
            size_t width = c == 'c' && s.width == SIZE_MAX ? 1 : s.width;
            size_t n = 0;
            while (n < width && p[n]) {
                unsigned char byte = p[n];
                if ((c == 's' && space(byte)) || (c == '[' && s.set[byte] == s.invert)) break;
                ++n;
            }
            if (!n) return assignments;
            if (c == 'c' && n != width) return assignments;
            if (!s.suppress) {
                char *out = va_arg(args, char *);
                memcpy(out, p, n);
                if (c != 'c') out[n] = '\0';
            }
            p += n;
        }
        completed = true;
        if (!s.suppress) ++assignments;
    }
    return assignments;
}
} // namespace

extern "C" int vsscanf(const char *input, const char *format, va_list source) {
    va_list copy;
    va_copy(copy, source);
    int result = scan(input, format, copy);
    va_end(copy);
    return result;
}
extern "C" int sscanf(const char *input, const char *format, ...) {
    va_list args;
    va_start(args, format);
    int result = vsscanf(input, format, args);
    va_end(args);
    return result;
}
