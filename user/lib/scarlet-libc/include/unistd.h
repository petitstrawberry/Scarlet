#ifndef SCARLET_UNISTD_H
#define SCARLET_UNISTD_H
#include <stddef.h>
#include <sys/types.h>

#define STDIN_FILENO 0
#define STDOUT_FILENO 1
#define STDERR_FILENO 2
#define F_OK 0
#define X_OK 1
#define W_OK 2
#define R_OK 4
#define _SC_PAGESIZE 30
#define _SC_PAGE_SIZE _SC_PAGESIZE
#define _SC_GETPW_R_SIZE_MAX 70
#define _POSIX_THREADS 200809L

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
int symlink(const char *, const char *);
int link(const char *, const char *);
char *getcwd(char *, size_t);
off_t lseek(int, off_t, int);
int dup(int);
int fsync(int);
int fdatasync(int);
int access(const char *, int);
ssize_t readlink(const char *, char *, size_t);
pid_t getpid(void);
pid_t getppid(void);
pid_t getpgid(pid_t);
pid_t getsid(pid_t);
uid_t getuid(void);
uid_t geteuid(void);
gid_t getgid(void);
gid_t getegid(void);
long sysconf(int);
#ifdef __cplusplus
}
#endif
#endif
