// Opt-in adapter for native CRTs that do not yet invoke __scarlet_c_finish.
// It returns to the real native CRT, which owns TLS cleanup and termination.
extern int __real_main(int, char**);
extern void __scarlet_c_finish(void);

int __wrap_main(int argc, char** argv) {
    int status = __real_main(argc, argv);
    __scarlet_c_finish();
    return status;
}
