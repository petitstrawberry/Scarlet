#include <android/log.h>
#include <dlfcn.h>
#include <stdio.h>
static void stderr_logger(const struct __android_log_message *m) {
    fprintf(stderr, "ANDROID_LOG %d %s: %s\n", m->priority, m->tag ? m->tag : "", m->message ? m->message : "");
}
void __android_log_set_logger(__android_logger_function logger) {
    (void)logger;
    void (*setter)(__android_logger_function) = dlsym(RTLD_NEXT, "__android_log_set_logger");
    if (setter) setter(stderr_logger);
}
void __android_log_logd_logger(const struct __android_log_message *message) {
    stderr_logger(message);
}
__attribute__((constructor)) static void install_logger(void) {
    __android_log_set_logger(stderr_logger);
}
