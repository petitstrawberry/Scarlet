#define _GNU_SOURCE
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/syscall.h>
#include <sys/wait.h>
#include <unistd.h>

/* Run with run-direct.py --program. Exercise ART's cache synchronization on
 * an RX alias before its first access, not just execution of a warm page. */
static void flush(void *address) {
    __builtin___clear_cache(address, (char *)address + 8);
}

static void *expected_fault;

static void fault_handler(int signal, siginfo_t *info, void *context) {
    (void)context;
    _exit(signal == SIGSEGV && info->si_code == SEGV_ACCERR &&
          info->si_addr == expected_fault ? 77 : 78);
}

static int denied(char *address, int store) {
    pid_t child = fork();
    if (child < 0) return 0;
    if (child == 0) {
        expected_fault = address;
        struct sigaction action = {.sa_sigaction = fault_handler, .sa_flags = SA_SIGINFO};
        sigemptyset(&action.sa_mask);
        if (sigaction(SIGSEGV, &action, NULL)) _exit(98);
        if (store) *(volatile char *)address = 0;
        else flush(address);
        _exit(99);
    }
    int status = 0;
    return waitpid(child, &status, 0) == child && WIFEXITED(status) &&
        WEXITSTATUS(status) == 77;
}

int main(void) {
    const size_t page = (size_t)sysconf(_SC_PAGESIZE);
    const uint32_t code[] = {0x52800540u, 0xd65f03c0u}; /* return 42 */
    int fd = (int)syscall(SYS_memfd_create, "jit-cache-regression", 0);
    if (fd < 0 || ftruncate(fd, 2 * page)) return 1;
    char *rw = mmap(NULL, page, PROT_READ | PROT_WRITE, MAP_SHARED, fd, page);
    char *rx = mmap(NULL, page, PROT_READ | PROT_EXEC, MAP_SHARED, fd, page);
    if (rw == MAP_FAILED || rx == MAP_FAILED) return 2;
    memcpy(rw + 0x3c0, code, sizeof(code));
    flush(rw + 0x3c0);
    puts("SCARLET_JIT_CACHE_COLD_RX");
    fflush(stdout);
    flush(rx + 0x3c0);
    if (((int (*)(void))(rx + 0x3c0))() != 42) return 3;
    if (!denied(rx + 0x3c0, 1)) return 4;
    if (mprotect(rx, page, PROT_NONE) || !denied(rx + 0x3c0, 0)) return 5;
    if (mprotect(rx, page, PROT_READ | PROT_EXEC)) return 6;
    flush(rx + 0x3c0);
    if (((int (*)(void))(rx + 0x3c0))() != 42) return 7;
    munmap(rx, page);
    munmap(rw, page);
    close(fd);
    puts("SCARLET_JIT_CACHE_OK");
    return 0;
}
