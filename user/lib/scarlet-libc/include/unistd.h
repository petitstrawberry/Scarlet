#ifndef SCARLET_UNISTD_H
#define SCARLET_UNISTD_H
#include <stddef.h>
#include <sys/types.h>

#define STDIN_FILENO 0
#define STDOUT_FILENO 1
#define STDERR_FILENO 2

#ifndef SEEK_SET
#define SEEK_SET 0
#define SEEK_CUR 1
#define SEEK_END 2
#endif

#ifdef __cplusplus
extern "C" {
#endif
int close(int);
ssize_t read(int, void *, size_t);
ssize_t write(int, const void *, size_t);
ssize_t pread(int, void *, size_t, off_t);
ssize_t pwrite(int, const void *, size_t, off_t);
int ftruncate(int, off_t);
int unlink(const char *);
int rmdir(const char *);
char *getcwd(char *, size_t);
off_t lseek(int, off_t, int);
int dup(int);
int fsync(int);
int fdatasync(int);
#ifdef __cplusplus
}
#endif
#endif
