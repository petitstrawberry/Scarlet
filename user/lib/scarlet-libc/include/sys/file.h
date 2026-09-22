#ifndef SCARLET_SYS_FILE_H
#define SCARLET_SYS_FILE_H
#define LOCK_SH 1
#define LOCK_EX 2
#define LOCK_NB 4
#define LOCK_UN 8
#ifdef __cplusplus
extern "C" {
#endif
/* Locks belong to an open file description and are shared by dup. Acquisition
   currently requires LOCK_NB; blocking acquisition returns ENOTSUP. */
int flock(int, int);
#ifdef __cplusplus
}
#endif
#endif
