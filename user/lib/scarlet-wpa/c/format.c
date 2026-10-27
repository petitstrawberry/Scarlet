/* Bounded diagnostic formatting for the embedded build. No floating point,
 * locale, streams or %n; unsupported formats return an error, not partial
 * success. */
#include "includes.h"
#include <limits.h>
#include <stdint.h>
static void put(char *out, size_t capacity, size_t *used, char value) {
  if (capacity && *used < capacity - 1)
    out[*used] = value;
  (*used)++;
}
int swpa_format(char *out, size_t capacity, const char *format, ...) {
  va_list args;
  va_start(args, format);
  size_t used = 0;
  while (*format) {
    if (*format != '%') {
      put(out, capacity, &used, *format++);
      continue;
    }
    format++;
    if (*format == '%') {
      put(out, capacity, &used, *format++);
      continue;
    }
    char pad = ' ';
    unsigned width = 0;
    int length = 0;
    if (*format == '0') {
      pad = '0';
      format++;
    }
    while (*format >= '0' && *format <= '9') {
      width = width * 10 + (*format++ - '0');
      if (width > 1024)
        goto error;
    }
    if (*format == 'z') {
      length = 3;
      format++;
    } else if (*format == 'l') {
      length = 1;
      format++;
      if (*format == 'l') {
        length = 2;
        format++;
      }
    }
    char kind = *format++;
    char number[32];
    const char *text = number;
    size_t count = 0;
    if (kind == 's') {
      text = va_arg(args, const char *);
      if (!text)
        text = "(null)";
      count = strlen(text);
    } else if (kind == 'c') {
      number[0] = (char)va_arg(args, int);
      count = 1;
    } else if (kind == 'u' || kind == 'd' || kind == 'i' || kind == 'x' ||
               kind == 'X' || kind == 'p') {
      uint64_t value;
      int negative = 0;
      if (kind == 'p') {
        value = (uintptr_t)va_arg(args, void *);
      } else if (kind == 'd' || kind == 'i') {
        int64_t signed_value = length == 3   ? (int64_t)va_arg(args, ptrdiff_t)
                               : length == 2 ? va_arg(args, long long)
                               : length == 1 ? va_arg(args, long)
                                             : va_arg(args, int);
        negative = signed_value < 0;
        value = negative ? 0 - (uint64_t)signed_value : (uint64_t)signed_value;
      } else {
        value = length == 3   ? va_arg(args, size_t)
                : length == 2 ? va_arg(args, unsigned long long)
                : length == 1 ? va_arg(args, unsigned long)
                              : va_arg(args, unsigned int);
      }
      unsigned base = (kind == 'x' || kind == 'X' || kind == 'p') ? 16 : 10;
      const char *digits =
          kind == 'X' ? "0123456789ABCDEF" : "0123456789abcdef";
      do {
        number[count++] = digits[value % base];
        value /= base;
      } while (value);
      if (negative)
        number[count++] = '-';
      for (size_t i = 0; i < count / 2; i++) {
        char c = number[i];
        number[i] = number[count - 1 - i];
        number[count - 1 - i] = c;
      }
      if (negative && pad == '0') {
        put(out, capacity, &used, '-');
        text++;
        count--;
        if (width)
          width--;
      }
    } else {
      goto error;
    }
    while (width > count) {
      put(out, capacity, &used, pad);
      width--;
    }
    for (size_t i = 0; i < count; i++)
      put(out, capacity, &used, text[i]);
  }
  if (capacity)
    out[used < capacity ? used : capacity - 1] = 0;
  va_end(args);
  return used > INT_MAX ? -1 : (int)used;
error:
  if (capacity)
    out[used < capacity ? used : capacity - 1] = 0;
  va_end(args);
  return -1;
}
