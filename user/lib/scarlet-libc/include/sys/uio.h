#ifndef SCARLET_SYS_UIO_H
#define SCARLET_SYS_UIO_H
#include <sys/socket.h>
#ifdef __cplusplus
extern "C" {
#endif
ssize_t readv(int, const struct iovec *, int);
ssize_t writev(int, const struct iovec *, int);
#ifdef __cplusplus
}
#endif
#endif
