/* Real execve/loader regression for Wine's 195-argument postinst command. */
typedef unsigned long usize;
typedef long isize;

static isize sc(usize n, usize a, usize b, usize c, usize d, usize e, usize f)
{
    register usize x8 __asm__("x8") = n;
    register usize x0 __asm__("x0") = a;
    register usize x1 __asm__("x1") = b;
    register usize x2 __asm__("x2") = c;
    register usize x3 __asm__("x3") = d;
    register usize x4 __asm__("x4") = e;
    register usize x5 __asm__("x5") = f;
    __asm__ volatile("svc 0" : "+r"(x0) : "r"(x8), "r"(x1), "r"(x2),
                     "r"(x3), "r"(x4), "r"(x5) : "memory", "cc");
    return (isize)x0;
}

static unsigned failures;
static char long_argument[8193];
static char oversized[131073];
static char *arguments[258], *environment[258];

static void print(const char *text)
{
    usize length = 0;
    while (text[length]) ++length;
    sc(64, 1, (usize)text, length, 0, 0, 0);
}

static void expect(const char *name, isize got, isize wanted)
{
    print(got == wanted ? "PASS " : "FAIL ");
    print(name);
    print("\n");
    if (got != wanted) ++failures;
}

static int equal(const char *a, const char *b)
{
    while (*a && *a == *b) { ++a; ++b; }
    return *a == *b;
}

static usize number(const char *text)
{
    usize value = 0;
    while (*text >= '0' && *text <= '9') value = value * 10 + *text++ - '0';
    return value;
}

static void finish(void)
{
    sc(94, failures ? 1 : 0, 0, 0, 0, 0, 0);
    for (;;) {}
}

static void verify(usize argc, char **argv, char **envp)
{
    expect("new image receives complete argc", argc, number(argv[2]));
    expect("argv terminator", (usize)argv[argc], 0);
    unsigned valid = equal(argv[0], "/var/tmp/exec-args-probe");
    for (usize i = 4; i + 1 < argc; ++i) valid &= equal(argv[i], "item");
    expect("all argument values survive exec", valid, 1);
    usize length = 0;
    while (argv[argc - 1][length] == 'x') ++length;
    expect("argument longer than pathname limit survives", length, 8192);
    expect("long argument terminator", argv[argc - 1][length], 0);
    usize count = 0;
    valid = 1;
    while (envp[count]) {
        valid &= equal(envp[count], "EXEC=1");
        ++count;
    }
    expect("new image receives complete envp", count, number(argv[3]));
    expect("all environment values survive exec", valid, 1);
    finish();
}

static void successful_exec(unsigned argc, unsigned envc, char *argc_text, char *envc_text)
{
    for (unsigned i = 0; i < argc; ++i) arguments[i] = "item";
    arguments[0] = "/var/tmp/exec-args-probe";
    arguments[1] = "--verify";
    arguments[2] = argc_text;
    arguments[3] = envc_text;
    arguments[argc - 1] = long_argument;
    arguments[argc] = 0;
    for (unsigned i = 0; i < envc; ++i) environment[i] = "EXEC=1";
    environment[envc] = 0;
    isize child = sc(220, 17, 0, 0, 0, 0, 0);
    if (child == 0) {
        sc(221, (usize)arguments[0], (usize)arguments, (usize)environment, 0, 0, 0);
        print("FAIL exec rejected valid argument vector\n");
        ++failures;
        finish();
    }
    expect("fork for real argument-vector exec", child > 0, 1);
    if (child > 0) {
        int status = -1;
        expect("wait for new image", sc(260, child, (usize)&status, 0, 0, 0, 0), child);
        expect("new image checks succeeded", status, 0);
    }
}

void start(usize *stack)
{
    usize argc = stack[0];
    char **argv = (char **)&stack[1];
    char **envp = &argv[argc + 1];
    if (argc >= 4 && equal(argv[1], "--verify")) verify(argc, argv, envp);

    for (usize i = 0; i < 8192; ++i) long_argument[i] = 'x';
    successful_exec(195, 80, "195", "80");
    successful_exec(256, 256, "256", "256");

    for (unsigned i = 0; i < 257; ++i) arguments[i] = "item";
    arguments[257] = 0;
    expect("too many arguments returns E2BIG", sc(221, (usize)"/bin/true", (usize)arguments, 0, 0, 0, 0), -7);
    arguments[0] = "true";
    arguments[1] = 0;
    for (unsigned i = 0; i < 257; ++i) environment[i] = "EXEC=1";
    environment[257] = 0;
    expect("too many environment entries returns E2BIG", sc(221, (usize)"/bin/true", (usize)arguments, (usize)environment, 0, 0, 0), -7);
    expect("bad argv pointer returns EFAULT", sc(221, (usize)"/bin/true", 1, 0, 0, 0, 0), -14);
    expect("bad argv string pointer returns EFAULT", sc(221, (usize)"/bin/true", (usize)(char *[]){(char *)1, 0}, 0, 0, 0, 0), -14);
    expect("bad envp pointer returns EFAULT", sc(221, (usize)"/bin/true", (usize)arguments, 1, 0, 0, 0), -14);
    expect("empty executable returns ENOENT", sc(221, (usize)"", (usize)arguments, 0, 0, 0, 0), -2);
    expect("bad executable pointer returns EFAULT", sc(221, 1, (usize)arguments, 0, 0, 0, 0), -14);
    expect("overlong executable path returns ENAMETOOLONG", sc(221, (usize)long_argument, (usize)arguments, 0, 0, 0, 0), -36);

    for (usize i = 0; i < 131072; ++i) oversized[i] = 'x';
    arguments[1] = oversized;
    arguments[2] = 0;
    expect("oversized string returns E2BIG", sc(221, (usize)"/bin/true", (usize)arguments, 0, 0, 0, 0), -7);
    oversized[131060] = 0;
    environment[0] = "EXEC=123456789";
    environment[1] = 0;
    expect("argv and envp share one size budget", sc(221, (usize)"/bin/true", (usize)arguments, (usize)environment, 0, 0, 0), -7);
    print(failures ? "Exec argument regression FAILED\n" : "Exec argument regression PASSED\n");
    finish();
}

__asm__(".global _start\n"
        "_start:\n"
        "mov x0, sp\n"
        "b start\n");
