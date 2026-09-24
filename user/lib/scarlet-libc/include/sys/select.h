#ifndef SCARLET_SYS_SELECT_H
#define SCARLET_SYS_SELECT_H

#include <sys/time.h>
#include <sys/types.h>

#define FD_SETSIZE 1024
typedef struct {
    unsigned long fds_bits[FD_SETSIZE / (8 * sizeof(unsigned long))];
} fd_set;

static inline void FD_ZERO(fd_set *set) {
    for (size_t i = 0; i < FD_SETSIZE / (8 * sizeof(unsigned long)); ++i)
        set->fds_bits[i] = 0;
}
static inline void FD_SET(int fd, fd_set *set) {
    if (fd >= 0 && fd < FD_SETSIZE)
        set->fds_bits[fd / (8 * sizeof(unsigned long))] |=
            1UL << (fd % (8 * sizeof(unsigned long)));
}
static inline void FD_CLR(int fd, fd_set *set) {
    if (fd >= 0 && fd < FD_SETSIZE)
        set->fds_bits[fd / (8 * sizeof(unsigned long))] &=
            ~(1UL << (fd % (8 * sizeof(unsigned long))));
}
static inline int FD_ISSET(int fd, const fd_set *set) {
    return fd >= 0 && fd < FD_SETSIZE &&
        (set->fds_bits[fd / (8 * sizeof(unsigned long))] &
         (1UL << (fd % (8 * sizeof(unsigned long))))) != 0;
}

#ifdef __cplusplus
extern "C" {
#endif
int select(int, fd_set *, fd_set *, fd_set *, struct timeval *);
#ifdef __cplusplus
}
#endif
#endif
