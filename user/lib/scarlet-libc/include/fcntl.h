#ifndef SCARLET_FCNTL_H
#define SCARLET_FCNTL_H
#include <sys/types.h>

#define O_ACCMODE 3
#define O_RDONLY 0
#define O_WRONLY 1
#define O_RDWR 2
#define O_CREAT 0x40
#define O_EXCL 0x80
#define O_TRUNC 0x200
#define O_APPEND 0x400
#define O_DIRECTORY 0x10000
#define O_NOFOLLOW 0x20000
#define O_CLOEXEC 0x80000
#define F_GETFD 1
#define F_SETFD 2
#define F_GETFL 3
#define F_SETFL 4
#define FD_CLOEXEC 1
#define AT_FDCWD (-100)
#define AT_SYMLINK_NOFOLLOW 0x100

#ifdef __cplusplus
extern "C" {
#endif
int open(const char *, int, ...);
int openat(int, const char *, int, ...);
int creat(const char *, mode_t);
int fcntl(int, int, ...);
#ifdef __cplusplus
}
#endif
#endif
