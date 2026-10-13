// Acceptance of the public native startup adapter and real C/C++ ABI symbols.
#include <errno.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

extern "C" int __cxa_atexit(void (*)(void*), void*, void*);
extern "C" void __cxa_finalize(void*);

namespace {
int callbacks;
int failures;
void check(const char* name, bool good) {
    printf("SCARLET_NATIVE_STARTUP %s %s\n", name, good ? "PASS" : "FAIL");
    if (!good) ++failures;
}
void first() {
    if (callbacks != 1) { puts("SCARLET_NATIVE_STARTUP atexit_lifo FAIL"); _Exit(1); }
    ++callbacks;
    puts("SCARLET_NATIVE_STARTUP atexit_lifo PASS");
}
void second() {
    if (callbacks != 0) { puts("SCARLET_NATIVE_STARTUP atexit_second FAIL"); _Exit(1); }
    ++callbacks;
    puts("SCARLET_NATIVE_STARTUP atexit_second PASS");
}
struct Global {
    ~Global() {
        if (callbacks != 2 || failures) {
            puts("SCARLET_NATIVE_STARTUP global_destructor FAIL");
            _Exit(1);
        }
        puts("SCARLET_NATIVE_STARTUP global_destructor PASS");
        puts("SCARLET_NATIVE_STARTUP RESULT failures=0");
    }
} global;

struct DsoState { char order[4] = {}; unsigned count = 0; };
void markA(void* argument) {
    auto* state = static_cast<DsoState*>(argument);
    state->order[state->count++] = 'A';
}
void markC(void* argument) {
    auto* state = static_cast<DsoState*>(argument);
    state->order[state->count++] = 'C';
}
void markB(void* argument) {
    auto* state = static_cast<DsoState*>(argument);
    state->order[state->count++] = 'B';
    if (__cxa_atexit(markC, state, state) != 0) _Exit(1);
    __cxa_finalize(state);
}
}

int main() {
    char* end = nullptr;
    const char* decimal = " \t-12.5tail";
    errno = 0;
    check("strtod_decimal_end", strtod(decimal, &end) == -12.5 && !strcmp(end, "tail") && !errno);
    check("strtof_hex_end", strtof("0x1.8p+1!", &end) == 3.0f && !strcmp(end, "!"));
    check("strtold", strtold("1.25/", &end) == 1.25L && !strcmp(end, "/"));
    const char* invalid = "unchanged";
    check("strtod_no_conversion", strtod(invalid, &end) == 0 && end == invalid);
    errno = 0;
    (void)strtod("1e9999", &end);
    check("strtod_overflow", errno == ERANGE && !*end);
    errno = 0;
    (void)strtof("1e-9999", &end);
    check("strtof_underflow", errno == ERANGE && !*end);

    DsoState state;
    check("dso_registration", __cxa_atexit(markA, &state, &state) == 0
        && __cxa_atexit(markB, &state, &state) == 0);
    __cxa_finalize(&state);
    __cxa_finalize(&state);
    check("dso_recursive_once", state.count == 3 && !strcmp(state.order, "BCA"));
    check("atexit_registration", atexit(first) == 0 && atexit(second) == 0);
    puts("SCARLET_NATIVE_STARTUP MAIN_RETURN");
    return failures ? 1 : 0;
}
