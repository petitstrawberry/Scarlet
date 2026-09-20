/* Scarlet Native syscalls only. No libc, Linux startup or Linux syscalls. */
typedef unsigned long word;
extern int answer(void);
extern void *dlopen(const char *, int);
extern void *dlsym(void *, const char *);
extern int dlclose(void *);
extern char *dlerror(void);

static word native_call(word number, word value) {
#if defined(__aarch64__)
    register word x0 __asm__("x0") = value;
    register word x8 __asm__("x8") = number;
    __asm__ volatile("svc #0" : "+r"(x0) : "r"(x8) : "memory");
    return x0;
#elif defined(__riscv) && __riscv_xlen == 64
    register word a0 __asm__("a0") = value;
    register word a7 __asm__("a7") = number;
    __asm__ volatile("ecall" : "+r"(a0) : "r"(a7) : "memory");
    return a0;
#else
#error unsupported smoke architecture
#endif
}
static void print(const char *message) {
    while (*message) native_call(16, (unsigned char)*message++);
}
__attribute__((noreturn)) static void fail(const char *reason) {
    print("SCARLET_LOADER_SMOKE_FAIL: "); print(reason); print("\n");
    native_call(1, 1);
    for (;;) native_call(21, 0);
}
int executable_value(void) { return 7; }
__attribute__((noreturn)) void _start(word argc, char **argv) {
    if (!argc || !argv || !argv[0] || argv[argc]) fail("startup arguments");
    if (answer() != 42) fail("startup dependency/constructors/weak relocation");
    print("SCARLET_LOADER_STARTUP_OK\n");
    if (dlopen("/system/lib/libsmoke-plugin.so", 2) || !dlerror())
        fail("unsupported local scope accepted");
    if (dlopen("/system/lib/missing-fixture.so", 0x102) || !dlerror())
        fail("missing library");
    void *process = dlopen((const char *)0, 0x102);
    if (!process) fail("process handle");
    void *handle = dlopen("/system/lib/libsmoke-plugin.so", 0x102);
    if (!handle) { char *error = dlerror(); fail(error ? error : "dlopen"); }
    int (*function)(void) = (int (*)(void))dlsym(handle, "plugin_answer");
    if (dlsym(process, "plugin_answer") != (void *)function)
        fail("process handle global scope");
    if (!function || function() != 50) fail("runtime symbol scope/constructor");
    if (dlsym(handle, "definitely_missing_symbol")) fail("missing symbol resolved");
    if (!dlerror() || dlerror()) fail("dlerror consumption");
    void *again = dlopen("/system/lib/libsmoke-plugin.so", 0x102);
    if (!again || function() != 50) fail("duplicate constructor");
    if (dlclose(again) || dlclose(handle)) fail("dlclose");
    if (dlsym(handle, "plugin_answer") || !dlerror()) fail("closed handle resolved");
    if (dlclose(handle) == 0 || !dlerror()) fail("closed handle reused");
    if (function() != 50) fail("closed object not pinned");
    if (dlclose(process)) fail("process handle close");
#ifdef SCARLET_SMOKE_RUST_DSO
    void *rust = dlopen("/system/lib/libsmoke-rust.so", 0x102);
    if (!rust) { char *error = dlerror(); fail(error ? error : "Rust cdylib open"); }
    int (*rust_answer)(void) = (int (*)(void))dlsym(rust, "rust_answer");
    if (!rust_answer || rust_answer() != 42) fail("Rust cdylib call");
    if (dlclose(rust)) fail("Rust cdylib close");
    print("SCARLET_LOADER_RUST_DSO_OK\n");
#endif
    print("SCARLET_LOADER_SMOKE_OK\n");
    native_call(1, 0);
    for (;;) native_call(21, 0);
}
