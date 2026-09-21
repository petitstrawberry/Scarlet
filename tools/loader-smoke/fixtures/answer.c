extern int dependency_answer(void);
extern int absent_weak(void) __attribute__((weak));
static int adjustment = 1;
static int *volatile adjustment_pointer = &adjustment;
static int initialized_answer;
int (*volatile dependency_pointer)(void) = dependency_answer;
__attribute__((constructor)) static void initialize_answer(void) {
    initialized_answer = dependency_pointer() + 1;
}
int answer(void) {
    return initialized_answer + *adjustment_pointer + (absent_weak ? 1000 : 0);
}
