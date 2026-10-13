#include <errno.h>
#include <inttypes.h>
#include <limits.h>
#include <locale.h>
#include <setjmp.h>
#include <stdarg.h>
#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <stdlib.h>
#include <pthread.h>
#include <sys/stat.h>
#include <unistd.h>

static int failures;
#define CHECK(name, expression) do { \
    int ok = !!(expression); \
    printf("WEBCORE_RUNTIME %s %s\n", (name), ok ? "PASS" : "FAIL"); \
    if (!ok) ++failures; \
} while (0)
extern int test_callee_saved(void);

static void jump_from_depth(jmp_buf target, int value, unsigned depth) {
    volatile unsigned mark = depth;
    if (depth) jump_from_depth(target, value, depth - 1);
    (void)mark;
    longjmp(target, value);
}
static void test_jumps(void) {
    jmp_buf target;
    volatile int initial = 0;
    if (setjmp(target) == 0) initial = 1;
    CHECK("setjmp_initial_zero", initial == 1);
    volatile int phase = 0;
    int result = setjmp(target);
    if (!phase) { phase = 1; jump_from_depth(target, 0, 4); }
    CHECK("longjmp_zero_becomes_one", result == 1 && phase == 1);
    phase = 0;
    result = setjmp(target);
    if (!phase) { phase = 1; jump_from_depth(target, -43, 5); }
    CHECK("longjmp_negative_return", result == -43 && phase == 1);
    jmp_buf outer, inner;
    phase = 0;
    result = setjmp(outer);
    if (!phase) {
        int inner_result = setjmp(inner);
        if (!inner_result) jump_from_depth(inner, 27, 3);
        phase = inner_result;
        jump_from_depth(outer, 91, 2);
    }
    CHECK("nested_jump_buffers", result == 91 && phase == 27);
    CHECK("gpr_simd_sp_restore", test_callee_saved() == 0);
}

static int via_vsscanf(const char *input, const char *format, ...) {
    va_list args;
    va_start(args, format);
    int result = vsscanf(input, format, args);
    va_end(args);
    return result;
}
static void test_scan(void) {
    int a = 0, b = 0, n = -1;
    char ch = 0, word[32] = {0};
    CHECK("scan_whitespace_integer", sscanf(" \t\n-42  17", " %d %d", &a, &b) == 2 && a == -42 && b == 17);
    CHECK("scan_width", sscanf("1234abcdef", "%2d%2d%3s%n", &a, &b, word, &n) == 3 && a == 12 && b == 34 && !strcmp(word,"abc") && n == 7);
    CHECK("scan_suppression_count", sscanf("44 skip 25!", "%*d %*s %d%c%n", &a, &ch, &n) == 2 && a == 25 && ch == '!' && n == 11);
    CHECK("scan_suppressed_large_integer", sscanf("999999999999999999999999999999 25", "%*d%d", &a) == 1 && a == 25);
    CHECK("scan_percent_count", sscanf("% 8", "%% %d", &a) == 1 && a == 8);
    CHECK("scan_percent_skips_space", sscanf(" \t%8", "%%%d", &a) == 1 && a == 8);
    CHECK("scan_percent_alone", sscanf("%", "%%") == 0);
    CHECK("scan_literal_mismatch", sscanf("ax", "ab%d", &a) == 0);
    CHECK("scan_empty_eof", sscanf("", "%d", &a) == EOF);
    CHECK("scan_space_eof", sscanf("  \t", "%d", &a) == EOF);
    CHECK("scan_partial_assignments", sscanf("21", "%d%d", &a, &b) == 1 && a == 21);
    CHECK("scan_suppressed_then_end", sscanf("21", "%*d%d", &a) == 0);
    CHECK("scan_n_empty", sscanf("", "%n", &n) == 0 && n == 0);
    CHECK("scan_n_then_end", sscanf("", "%n%d", &n, &a) == 0 && n == 0);
    CHECK("scan_sign_failure", sscanf("+", "%d", &a) == 0);
    CHECK("scan_base_auto", sscanf("0x2a 077 -23", "%i%i%i", &a, &b, &n) == 3 && a == 42 && b == 63 && n == -23);
    unsigned u=0, x=0, o=0;
    CHECK("scan_unsigned_bases", sscanf("-1 fF 77", "%u%x%o", &u, &x, &o) == 3 && u == UINT_MAX && x == 255 && o == 63);
    CHECK("scan_prefix_failure", sscanf("0x", "%x", &x) == 0);
    signed char hh=0; short h=0; long l=0; long long ll=0; intmax_t j=0; size_t z=0; ptrdiff_t t=0;
    CHECK("scan_integer_lengths", sscanf("-128 -32768 -2147483649 -9223372036854775808 77 88 -99", "%hhd%hd%ld%lld%jd%zu%td", &hh, &h, &l, &ll, &j, &z, &t) == 7 && hh == -128 && h == -32768 && l == -2147483649L && ll == LLONG_MIN && j == 77 && z == 88 && t == -99);
    unsigned long long ull=0;
    CHECK("scan_uint64_max", sscanf("18446744073709551615", "%llu", &ull) == 1 && ull == ULLONG_MAX);
    hh=7; errno=0;
    CHECK("scan_integer_range_rejected", sscanf("128", "%hhd", &hh) == 0 && hh == 7 && errno == ERANGE);
    void *pointer=0;
    CHECK("scan_pointer", sscanf("0x1234", "%p", &pointer) == 1 && (uintptr_t)pointer == 0x1234);
    memset(word, '?', sizeof(word));
    CHECK("scan_char_width_no_null", sscanf(" ab", "%2c", word) == 1 && word[0]==' ' && word[1]=='a' && word[2]=='?');
    memset(word, '?', sizeof(word));
    CHECK("scan_short_char_matching_failure", sscanf("a", "%2c", word) == 0 && word[0] == '?');
    CHECK("scan_scanset", sscanf("abc123Z", "%[a-z]%3[0-9]%c", word, word+8, &ch) == 3 && !strcmp(word,"abc") && !strcmp(word+8,"123") && ch=='Z');
    CHECK("scan_inverted_scanset", sscanf("one,two", "%[^,],%s", word, word+8) == 2 && !strcmp(word,"one") && !strcmp(word+8,"two"));
    CHECK("scan_bracket_scanset", sscanf("]-x", "%[]-]%c", word, &ch) == 2 && !strcmp(word,"]-") && ch=='x');
    float f=0; double d=0;
    CHECK("scan_float_decimal", sscanf("-1.25e2 .5", "%f%lf", &f, &d) == 2 && f == -125.0f && d == 0.5);
    CHECK("scan_float_hex", sscanf("0x1.8p+2", "%la%n", &d, &n) == 1 && d == 6.0 && n == 8);
    CHECK("scan_float_width", sscanf("12e3", "%2f%c", &f, &ch) == 2 && f == 12.0f && ch == 'e');
    CHECK("scan_float_bad_exponent", sscanf("100er", "%f", &f) == 0);
    CHECK("scan_float_bad_prefix", sscanf("0x", "%f", &f) == 0);
    CHECK("scan_float_inf_delimiter", sscanf("INF!", "%f%c%n", &f, &ch, &n) == 2 && __builtin_isinf(f) && ch=='!' && n==4);
    CHECK("scan_float_nan_delimiter", sscanf("nan(payload),", "%lf%c", &d, &ch) == 2 && __builtin_isnan(d) && ch==',');
    CHECK("scan_float_infinity", sscanf("-infinity!", "%lf%c", &d, &ch) == 2 && __builtin_isinf(d) && d<0 && ch=='!');
    CHECK("scan_float_partial_infinity", sscanf("infin!", "%lf", &d) == 0);
    CHECK("scan_float_rounding", sscanf("1.000000059604644775390625", "%f", &f) == 1 && f == 1.0f);
    CHECK("scan_float_suppress", sscanf("3.5 9", "%*f%d", &a) == 1 && a == 9);
    CHECK("scan_vsscanf", via_vsscanf("word:33", "%4s:%d", word, &a) == 2 && !strcmp(word,"word") && a==33);
    errno=0; a=17;
    // Unsupported directives reject the entire format before any assignment.
    CHECK("scan_wide_explicit_rejection", sscanf("1 a", "%d%ls", &a, (void*)word) == EOF && errno==ENOTSUP && a==17);
    errno=0;
    CHECK("scan_long_double_rejection", sscanf("1", "%Lf", (void*)word) == EOF && errno==ENOTSUP);
    errno=0;
    CHECK("scan_allocate_rejection", sscanf("x", "%ms", (void*)word) == EOF && errno==ENOTSUP);
    errno=0;
    CHECK("scan_positional_rejection", sscanf("1", "%1$d", &a) == EOF && errno==ENOTSUP);
    errno=0;
    CHECK("scan_bad_format_rejection", sscanf("1", "%0d", &a) == EOF && errno==EINVAL);
}
static void test_locale(void) {
    struct lconv *c=localeconv();
    CHECK("locale_decimal_point", c && !strcmp(c->decimal_point,"."));
    CHECK("locale_empty_strings", !*c->thousands_sep && !*c->grouping && !*c->int_curr_symbol && !*c->currency_symbol && !*c->mon_decimal_point && !*c->mon_thousands_sep && !*c->mon_grouping && !*c->positive_sign && !*c->negative_sign);
    CHECK("locale_char_max", c->int_frac_digits==CHAR_MAX && c->frac_digits==CHAR_MAX && c->p_cs_precedes==CHAR_MAX && c->p_sep_by_space==CHAR_MAX && c->n_cs_precedes==CHAR_MAX && c->n_sep_by_space==CHAR_MAX && c->p_sign_posn==CHAR_MAX && c->n_sign_posn==CHAR_MAX && c->int_p_cs_precedes==CHAR_MAX && c->int_p_sep_by_space==CHAR_MAX && c->int_n_cs_precedes==CHAR_MAX && c->int_n_sep_by_space==CHAR_MAX && c->int_p_sign_posn==CHAR_MAX && c->int_n_sign_posn==CHAR_MAX);
}
static void test_perror(void) {
    // Swap only this single-threaded probe's public stderr pointer. The native
    // stream implementation performs all file I/O, with no fd/syscall shim.
    const char *path="/tmp/webcore-runtime-perror.txt";
    FILE *capture=fopen(path,"w+");
    CHECK("perror_capture_open", capture != 0);
    if (!capture) return;
    char expected[512], actual[512];
    snprintf(expected,sizeof(expected),"probe: %s\n%s\n%s\n",strerror(ENOENT),strerror(EINVAL),strerror(EACCES));
    FILE *saved=stderr; stderr=capture;
    errno=ENOENT; perror("probe"); int preserved=errno==ENOENT;
    errno=EINVAL; perror(""); preserved &= errno==EINVAL;
    errno=EACCES; perror(0); preserved &= errno==EACCES;
    fflush(capture); stderr=saved;
    CHECK("perror_errno_preserved", preserved);
    CHECK("perror_capture_rewind", fseek(capture,0,SEEK_SET)==0);
    size_t n=fread(actual,1,sizeof(actual)-1,capture); actual[n]=0;
    CHECK("perror_native_strerror_text", !strcmp(actual,expected));
    fclose(capture); remove(path);
}
struct RandResult { uint64_t sum; unsigned int xors; int range_ok; };
static void *random_thread(void *argument) {
    struct RandResult *out=argument;
    out->sum=0; out->xors=0; out->range_ok=1;
    for (int i=0;i<1000;++i) {
        int n=rand(); out->sum+=(unsigned)n; out->xors^=(unsigned)n;
        if (n<0 || n>RAND_MAX) out->range_ok=0;
    }
    return argument;
}
static void test_random(void) {
    int initial=rand(); srand(1);
    CHECK("rand_default_seed", initial==rand());
    int sequence[8]; srand(12345);
    for (unsigned i=0;i<8;++i) sequence[i]=rand();
    srand(12345); int same=1;
    for (unsigned i=0;i<8;++i) if(sequence[i]!=rand()) same=0;
    CHECK("rand_seed_repeatable", same);
    srand(0); uint32_t state=0; int reference=1;
    for (unsigned i=0;i<16;++i) {
        state=state*UINT32_C(1664525)+UINT32_C(1013904223);
        if(rand()!=(int)(state>>1)) reference=0;
    }
    CHECK("rand_reference_sequence", reference && RAND_MAX==2147483647);
    struct RandResult first,second;
    srand(123); random_thread(&first); random_thread(&second);
    uint64_t expected_sum=first.sum+second.sum;
    unsigned expected_xor=first.xors^second.xors;
    int expected_next=rand();
    srand(123); pthread_t a,b;
    int a_ok=pthread_create(&a,0,random_thread,&first)==0;
    int b_ok=pthread_create(&b,0,random_thread,&second)==0;
    CHECK("rand_concurrent_start", a_ok && b_ok);
    void *a_result=0,*b_result=0;
    int a_join=a_ok ? pthread_join(a,&a_result) : -1;
    int b_join=b_ok ? pthread_join(b,&b_result) : -1;
    CHECK("rand_concurrent_join", a_join==0 && b_join==0 && a_result==&first && b_result==&second);
    if (a_ok && b_ok && !a_join && !b_join) {
        CHECK("rand_concurrent_sequence", first.range_ok && second.range_ok && first.sum+second.sum==expected_sum && (first.xors^second.xors)==expected_xor && rand()==expected_next);
    }
}
static void test_tmpfile(void) {
    FILE *one=tmpfile(),*two=tmpfile();
    CHECK("tmpfile_two_unique_streams", one && two && fileno(one)!=fileno(two));
    if (!one || !two) { if(one)fclose(one); if(two)fclose(two); return; }
    const char payload[]="native temporary stream"; char buffer[64]={0};
    CHECK("tmpfile_write_seek", fwrite(payload,1,sizeof(payload),one)==sizeof(payload) && fseek(one,0,SEEK_SET)==0);
    CHECK("tmpfile_readback", fread(buffer,1,sizeof(payload),one)==sizeof(payload) && !memcmp(buffer,payload,sizeof(payload)));
    CHECK("tmpfile_independent_empty", fread(buffer,1,1,two)==0 && feof(two));
    struct stat status;
    CHECK("tmpfile_unlinked_inode", fstat(fileno(one),&status)==0 && status.st_nlink==0);
    CHECK("tmpfile_close", fclose(one)==0 && fclose(two)==0);
}
int main(void) {
    test_jumps(); test_scan(); test_locale(); test_perror(); test_random(); test_tmpfile();
    printf("WEBCORE_RUNTIME RESULT failures=%d\n",failures);
    return failures ? 1 : 0;
}
