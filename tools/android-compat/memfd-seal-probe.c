/* Android shared-memory semantics: existing writers, RX aliases and fork. */
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/syscall.h>
#include <sys/wait.h>
#include <unistd.h>

#ifndef F_SEAL_FUTURE_WRITE
#define F_SEAL_FUTURE_WRITE 0x10
#endif
#define CHECK(test) do { if (!(test)) { perror(#test); return 1; } } while (0)

int main(void) {
    setvbuf(stdout, NULL, _IONBF, 0);
    size_t page = (size_t)sysconf(_SC_PAGESIZE);
    int fd = (int)syscall(__NR_memfd_create, "android-seal-probe", 3);
    CHECK(fd >= 0);
    CHECK(ftruncate(fd, (off_t)(3 * page)) == 0);
    unsigned char *writer = mmap(NULL, 3 * page, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
    CHECK(writer != MAP_FAILED);
    writer[page] = 0x5a;
    writer[2 * page] = 0xa5;
    int duplicate = dup(fd);
    CHECK(duplicate >= 0);
    CHECK(fcntl(fd, F_ADD_SEALS, F_SEAL_FUTURE_WRITE) == 0);
    CHECK(fcntl(duplicate, F_GET_SEALS) == F_SEAL_FUTURE_WRITE);
    errno = 0;
    CHECK(pwrite(duplicate, "x", 1, 0) == -1 && errno == EPERM);
    errno = 0;
    CHECK(mmap(NULL, page, PROT_READ | PROT_WRITE, MAP_SHARED, duplicate, 0) == MAP_FAILED && errno == EACCES);
    unsigned char *reader = mmap(NULL, 2 * page, PROT_READ | PROT_EXEC, MAP_SHARED, fd, (off_t)page);
    CHECK(reader != MAP_FAILED);
    CHECK(reader[0] == 0x5a && reader[page] == 0xa5);
    errno = 0;
    CHECK(mprotect(reader, page, PROT_READ | PROT_WRITE) == -1 && errno == EACCES);
    CHECK(mprotect(writer, page, PROT_READ) == 0);
    CHECK(mprotect(writer, page, PROT_READ | PROT_WRITE) == 0);
    unsigned char *cow = mmap(NULL, page, PROT_READ | PROT_WRITE, MAP_PRIVATE, fd, (off_t)page);
    CHECK(cow != MAP_FAILED);
    cow[0] = 0x11;
    CHECK(reader[0] == 0x5a);
    CHECK(close(fd) == 0 && close(duplicate) == 0);
    pid_t child = fork();
    CHECK(child >= 0);
    if (!child) {
        if (reader[0] != 0x5a || reader[page] != 0xa5) _exit(42);
        writer[page] = 0x77;
        _exit(0);
    }
    int status;
    CHECK(waitpid(child, &status, 0) == child && WIFEXITED(status) && !WEXITSTATUS(status));
    CHECK(reader[0] == 0x77);
    CHECK(munmap(reader, page) == 0);
    CHECK(reader[page] == 0xa5);
    CHECK(munmap(reader + page, page) == 0);
    CHECK(munmap(writer, 3 * page) == 0 && munmap(cow, page) == 0);
    puts("SCARLET_MEMFD_SEAL_OK existing-writer future-writer-denied rx-offset private-cow fork fd-close partial-unmap");
    return 0;
}
