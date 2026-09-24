#ifndef SCARLET_POLL_H
#define SCARLET_POLL_H

#include <stddef.h>

typedef size_t nfds_t;

struct pollfd {
    int fd;
    short events;
    short revents;
};

#define POLLIN   0x0001
#define POLLPRI  0x0002
#define POLLOUT  0x0004
#define POLLERR  0x0008
#define POLLHUP  0x0010
#define POLLNVAL 0x0020

#ifdef __cplusplus
extern "C" {
#endif
int poll(struct pollfd *, nfds_t, int);
#ifdef __cplusplus
}
#endif

#endif
