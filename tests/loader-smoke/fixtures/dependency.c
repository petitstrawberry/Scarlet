/* Freestanding fixture: its constructor must run before answer.c's. */
volatile int dependency_value;
__attribute__((constructor)) static void initialize_dependency(void) {
    dependency_value = 40;
}
int dependency_answer(void) { return dependency_value; }
