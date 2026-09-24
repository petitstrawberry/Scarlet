#ifndef SCARLET_PWD_H
#define SCARLET_PWD_H
#include <sys/types.h>
struct passwd {
    char *pw_name;
    char *pw_passwd;
    uid_t pw_uid;
    gid_t pw_gid;
    char *pw_gecos;
    char *pw_dir;
    char *pw_shell;
};
#ifdef __cplusplus
extern "C" {
#endif
int getpwuid_r(uid_t, struct passwd *, char *, size_t, struct passwd **);
#ifdef __cplusplus
}
#endif
#endif
