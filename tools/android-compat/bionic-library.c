static __thread int value = 100;
__attribute__((constructor)) static void initialize(void) { value++; }
int bionic_probe_value(void) { return value; }
