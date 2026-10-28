/* SPDX-License-Identifier: MIT */
/* A small static JNI launcher for the initial Scarlet OpenJDK port. */
#include <jni.h>
#include <pthread.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

extern jstring JNU_NewStringPlatform(JNIEnv *env, const char *text);

struct launch_args { int argc; char **argv; int result; };
static void *run_vm(void *data) {
    struct launch_args *args = data;
    JavaVM *vm = NULL;
    JNIEnv *env = NULL;
    JavaVMOption options[64];
    int n = 0, i = 1, version = 0;
    char *classpath_option = NULL;
    options[n++].optionString = "-Xms16m";
    options[n++].optionString = "-Xmx64m";
    options[n++].optionString = "-Xrs";
    options[n++].optionString = "-XX:+UseSerialGC";
    options[n++].optionString = "-XX:-UsePerfData";
    options[n++].optionString = "-Djava.class.path=.";
    while (i < args->argc && args->argv[i][0] == '-') {
        const char *option = args->argv[i++];
        if (n == 64) { fputs("Too many VM options\n", stderr); goto options_error; }
        if (!strcmp(option, "-version") || !strcmp(option, "--version")) {
            version = !strcmp(option, "-version") ? 1 : 2;
            break;
        }
        if (!strcmp(option, "-cp") || !strcmp(option, "-classpath") || !strcmp(option, "--class-path")) {
            if (i == args->argc) { fputs("Missing class path\n", stderr); goto options_error; }
            if (classpath_option) { fputs("Specify the class path once\n", stderr); goto options_error; }
            const char *path = args->argv[i++];
            classpath_option = malloc(strlen(path) + sizeof("-Djava.class.path="));
            if (!classpath_option) goto options_error;
            strcpy(classpath_option, "-Djava.class.path=");
            strcat(classpath_option, path);
            options[n++].optionString = classpath_option;
        } else if (!strcmp(option, "-jar") || !strcmp(option, "-m") || !strcmp(option, "--module")) {
            fputs("This Scarlet launcher currently accepts a main class; -jar/-m are unavailable\n", stderr);
            goto options_error;
        } else {
            options[n++].optionString = (char*)option;
        }
    }
    if (!version && i == args->argc) {
        fputs("Missing main class. Usage: java [VM options] [-cp path] Main [args...]\n", stderr);
        goto options_error;
    }
    for (int j = 0; j < n; ++j) options[j].extraInfo = NULL;
    JavaVMInitArgs init = { .version = JNI_VERSION_1_8, .nOptions = n,
        .options = options, .ignoreUnrecognized = JNI_FALSE };
    jint status = JNI_CreateJavaVM(&vm, (void **)&env, &init);
    free(classpath_option);
    classpath_option = NULL;
    if (status != JNI_OK) {
        fprintf(stderr, "JNI_CreateJavaVM failed: %d\n", status);
        return NULL;
    }
    if (version) {
        jclass props = (*env)->FindClass(env, "java/lang/VersionProps");
        if (!props) goto done;
        jmethodID print = (*env)->GetStaticMethodID(env, props, "print", "(Z)V");
        if (!print) goto done;
        (*env)->CallStaticVoidMethod(env, props, print, version == 1 ? JNI_TRUE : JNI_FALSE);
        if (!(*env)->ExceptionCheck(env)) args->result = 0;
        goto done;
    }
    const char *name = args->argv[i++];
    char *class_name = strdup(name);
    if (!class_name) goto done;
    for (char *p = class_name; *p; ++p) if (*p == '.') *p = '/';
    jclass main_class = (*env)->FindClass(env, class_name);
    free(class_name);
    if (!main_class) goto done;
    jmethodID main_method = (*env)->GetStaticMethodID(env, main_class, "main", "([Ljava/lang/String;)V");
    if (!main_method) goto done;
    jclass string_class = (*env)->FindClass(env, "java/lang/String");
    if (!string_class) goto done;
    jobjectArray java_args = (*env)->NewObjectArray(env, args->argc-i, string_class, NULL);
    if (!java_args) goto done;
    for (int j = 0; i < args->argc; ++i, ++j) {
        jstring value = JNU_NewStringPlatform(env, args->argv[i]);
        if (!value) goto done;
        (*env)->SetObjectArrayElement(env, java_args, j, value);
        (*env)->DeleteLocalRef(env, value);
        if ((*env)->ExceptionCheck(env)) goto done;
    }
    (*env)->CallStaticVoidMethod(env, main_class, main_method, java_args);
    if (!(*env)->ExceptionCheck(env)) args->result = 0;
done:
    if ((*env)->ExceptionCheck(env)) (*env)->ExceptionDescribe(env);
    if ((*vm)->DestroyJavaVM(vm) != JNI_OK) args->result = 1;
    return NULL;
options_error:
    free(classpath_option);
    return NULL;
}
extern void scarlet_jvm_initialize_runtime(void);
int main(int argc, char **argv) {
    if (argc == 1 || !strcmp(argv[1], "-h") || !strcmp(argv[1], "--help") || !strcmp(argv[1], "-help")) {
        puts("Usage: java [VM options] [-cp path] Main [args...]\n"
             "       java -version | --version\n"
             "Scarlet native OpenJDK: HotSpot, Serial GC, static JNI libraries.");
        return argc == 1 ? 1 : 0;
    }
    scarlet_jvm_initialize_runtime();
    struct launch_args args = { argc, argv, 1 };
    pthread_attr_t attr;
    pthread_t thread;
    if (pthread_attr_init(&attr) != 0) return 1;
    if (pthread_attr_setstacksize(&attr, 2*1024*1024) != 0) {
        pthread_attr_destroy(&attr);
        return 1;
    }
    int status = pthread_create(&thread, &attr, run_vm, &args);
    pthread_attr_destroy(&attr);
    if (status || pthread_join(thread, NULL)) return 1;
    return args.result;
}
