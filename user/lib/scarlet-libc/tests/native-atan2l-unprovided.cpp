// Negative link fixture: this API is declared for header parsing, not supplied
// by the native binary128 runtime. Real use must stay an unresolved symbol.
#include <complex>
#include <float.h>

static_assert(sizeof(long double) == 16 && LDBL_MANT_DIG == 113);
volatile long double value = -1.0L;

int main() {
    // Volatile input and complex's real long-double overload force the call.
    long double angle = std::arg(static_cast<long double>(value));
    return angle == 0;
}
