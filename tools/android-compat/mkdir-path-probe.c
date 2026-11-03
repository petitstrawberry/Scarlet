/* mkdirat must copy pathnames through every user page, including permissions. */
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/stat.h>
#include <sys/syscall.h>
#include <unistd.h>
#define CHECK(x) do { if (!(x)) { perror(#x); return 1; } } while (0)
int main(void) {
    size_t page = (size_t)sysconf(_SC_PAGESIZE);
    char *area = mmap(NULL, 2 * page, PROT_READ | PROT_WRITE,
                      MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    CHECK(area != MAP_FAILED);
    const char *name = "/tmp/scarlet-mkdir-page-test";
    char *crossing = area + page - 7;
    strcpy(crossing, name);
    CHECK(mprotect(area + page, page, PROT_NONE) == 0);
    errno = 0;
    CHECK(syscall(SYS_mkdirat, AT_FDCWD, crossing, 0700) == -1 && errno == EFAULT);
    CHECK(mprotect(area + page, page, PROT_READ | PROT_WRITE) == 0);
    CHECK(syscall(SYS_mkdirat, AT_FDCWD, crossing, 0700) == 0);
    struct stat st;
    CHECK(stat(name, &st) == 0 && S_ISDIR(st.st_mode));
    CHECK(chmod(crossing, 0751) == 0);
    CHECK(stat(name, &st) == 0 && (st.st_mode & 0777) == 0751);
    CHECK(rmdir(name) == 0);
    char *ending = area + page - strlen(name) - 1;
    strcpy(ending, name);
    CHECK(mprotect(area + page, page, PROT_NONE) == 0);
    CHECK(syscall(SYS_mkdirat, AT_FDCWD, ending, 0700) == 0);
    CHECK(rmdir(name) == 0);
    CHECK(munmap(area, 2 * page) == 0);
    puts("SCARLET_MKDIR_PATH_OK");
    return 0;
}
