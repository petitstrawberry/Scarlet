#ifndef SCARLET_ERRNO_H
#define SCARLET_ERRNO_H
#ifdef __cplusplus
extern "C" {
#endif
int *__errno_location(void);
#ifdef __cplusplus
}
#endif
#define errno (*__errno_location())
#define ENOENT 2
#define EINTR 4
#define EIO 5
#define EBADF 9
#define EAGAIN 11
#define EWOULDBLOCK EAGAIN
#define ENOMEM 12
#define EACCES 13
#define EFAULT 14
#define EBUSY 16
#define EEXIST 17
#define EXDEV 18
#define ENOTDIR 20
#define EISDIR 21
#define EINVAL 22
#define EMFILE 24
#define ENOSPC 28
#define ESPIPE 29
#define EROFS 30
#define EPIPE 32
#define ERANGE 34
#define ENAMETOOLONG 36
#define ENOTEMPTY 39
#define ELOOP 40
#define EOVERFLOW 75
#define ENOTSUP 95
#define EOPNOTSUPP ENOTSUP
#endif
