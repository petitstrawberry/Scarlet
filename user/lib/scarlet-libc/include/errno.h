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
#define EPERM 1
#define ENOENT 2
#define ESRCH 3
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
#define ENOTTY 25
#define ENOSPC 28
#define ESPIPE 29
#define EROFS 30
#define EPIPE 32
#define ERANGE 34
#define EDEADLK 35
#define ENAMETOOLONG 36
#define ENOLCK 37
#define ENOTEMPTY 39
#define ELOOP 40
#define EOVERFLOW 75
#define ENOTSOCK 88
#define EMSGSIZE 90
#define EPROTONOSUPPORT 93
#define ENOTSUP 95
#define EOPNOTSUPP ENOTSUP
#define EAFNOSUPPORT 97
#define EADDRINUSE 98
#define EADDRNOTAVAIL 99
#define ENETUNREACH 101
#define ECONNABORTED 103
#define ECONNRESET 104
#define EISCONN 106
#define ENOTCONN 107
#define ETIMEDOUT 110
#define ECONNREFUSED 111
#define EINPROGRESS 115
#endif
