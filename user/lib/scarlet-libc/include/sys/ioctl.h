#ifndef SCARLET_SYS_IOCTL_H
#define SCARLET_SYS_IOCTL_H

/* Match the Linux ioctl request used by portable socket libraries. */
#define FIONBIO 0x5421UL

#ifdef __cplusplus
extern "C" {
#endif
int ioctl(int, unsigned long, ...);
#ifdef __cplusplus
}
#endif

#endif
