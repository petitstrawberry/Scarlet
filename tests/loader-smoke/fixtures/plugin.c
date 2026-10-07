extern int answer(void);
extern int executable_value(void);
static volatile int calls;
__attribute__((constructor)) static void initialize_plugin(void) { calls++; }
int plugin_answer(void) { return answer() + executable_value() + calls; }
