#ifndef SCARLET_STATVFS_H
#define SCARLET_STATVFS_H
struct statvfs {
    unsigned long f_bsize, f_frsize;
    unsigned long long f_blocks, f_bfree, f_bavail;
    unsigned long long f_files, f_ffree, f_favail;
    unsigned long f_fsid, f_flag, f_namemax;
};
#ifdef __cplusplus
extern "C" {
#endif
/* Native filesystem capacity reporting is not available (ENOSYS). */
int statvfs(const char *, struct statvfs *);
int fstatvfs(int, struct statvfs *);
#ifdef __cplusplus
}
#endif
#endif
