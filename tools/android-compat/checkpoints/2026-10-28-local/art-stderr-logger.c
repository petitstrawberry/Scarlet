#include <android/log.h>
#include <stdio.h>
static void stderr_logger(const struct __android_log_message *m) {
    fprintf(stderr, "ANDROID_LOG %d %s: %s\n", m->priority, m->tag ? m->tag : "", m->message ? m->message : "");
}
__attribute__((constructor)) static void install_logger(void) {
    __android_log_set_logger(stderr_logger);
}
