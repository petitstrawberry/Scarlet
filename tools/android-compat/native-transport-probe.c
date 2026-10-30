#include <stdint.h>
#include <stdio.h>
#include <unistd.h>
#include <sys/wait.h>

extern uintptr_t scarlet_sdk_pid(void);
extern uintptr_t scarlet_sdk_pid_number(void);

static int check(void) {
    uintptr_t native = scarlet_sdk_pid();
    pid_t linux_pid = getpid();
    printf("SCARLET_ANDROID_NATIVE_TRANSPORT number=%#lx native_pid=%lu bionic_pid=%d\n",
           (unsigned long)scarlet_sdk_pid_number(), (unsigned long)native, linux_pid);
    return scarlet_sdk_pid_number() == 0x53430007 && native == (uintptr_t)linux_pid;
}

int main(void) {
    if (!check()) return 2;
    pid_t child = fork();
    if (child < 0) return 3;
    if (child == 0) _exit(check() ? 0 : 4);
    int status;
    if (waitpid(child, &status, 0) != child || !WIFEXITED(status) || WEXITSTATUS(status)) return 5;
    puts("SCARLET_ANDROID_NATIVE_TRANSPORT_OK");
    return 0;
}
