// Correctly rounded C-locale conversion using the matching LLVM libc core.
// LLVM support headers are supplied by the SDK's pinned LLVM source checkout.
#include <errno.h>
#include <stdlib.h>
#include "shared/str_to_float.h"

namespace {
template<class T> T convert(const char* text, char** end) {
    auto result = __llvm_libc::internal::strtofloatingpoint<T>(text);
    if (result.has_error()) errno = result.error;
    if (end) *end = const_cast<char*>(text + result.parsed_len);
    return result.value;
}
}

extern "C" float strtof(const char* text, char** end) {
    return convert<float>(text, end);
}
extern "C" double strtod(const char* text, char** end) {
    return convert<double>(text, end);
}
extern "C" long double strtold(const char* text, char** end) {
    return convert<long double>(text, end);
}
extern "C" double atof(const char* text) {
    return strtod(text, nullptr);
}
