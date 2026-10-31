// Minimal Android vold user-storage adapter for the Scarlet-hosted view.
//
// The outer Android manager creates and labels the user storage roots. This
// helper supplies vold's required process boundary and makes the fixed DE/CE
// hierarchy idempotently usable without changing host-managed SELinux labels.
#define _GNU_SOURCE
#include <dirent.h>
#include <errno.h>
#include <limits.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <unistd.h>

enum { STORAGE_FLAG_DE = 1, STORAGE_FLAG_CE = 2 };
enum { AID_ROOT = 0, AID_SYSTEM = 1000, AID_CACHE = 2001 };

static int ensure_dir(const char *path, mode_t mode, uid_t uid, gid_t gid) {
    char parent[PATH_MAX];
    if (strlen(path) >= sizeof(parent)) return ENAMETOOLONG;
    strcpy(parent, path);
    char *slash = strrchr(parent, '/');
    if (!slash || slash == parent) return EINVAL;
    *slash = '\0';
    struct stat st;
    if (lstat(parent, &st) != 0) {
        int error = ensure_dir(parent, 0755, AID_ROOT, AID_ROOT);
        if (error) return error;
    }
    if (mkdir(path, mode) != 0 && errno != EEXIST) return errno;
    if (lstat(path, &st) != 0) return errno;
    if (!S_ISDIR(st.st_mode)) return ENOTDIR;
    if (chown(path, uid, gid) != 0 || chmod(path, mode) != 0) return errno;
    return 0;
}

static int prepare_apex_data(const char *base, mode_t mode, uid_t owner, gid_t group) {
    char path[PATH_MAX];
    if (snprintf(path, sizeof(path), "%s/apexdata", base) >= (int)sizeof(path))
        return ENAMETOOLONG;
    int error = ensure_dir(path, 0711, AID_ROOT, AID_ROOT);
    if (error) return error;
    DIR *directory = opendir("/apex");
    if (!directory) return errno;
    struct dirent *entry;
    while ((entry = readdir(directory)) != NULL) {
        if (entry->d_name[0] == '.' || strchr(entry->d_name, '@')) continue;
        if (snprintf(path, sizeof(path), "%s/apexdata/%s", base, entry->d_name)
            >= (int)sizeof(path)) {
            closedir(directory);
            return ENAMETOOLONG;
        }
        error = ensure_dir(path, mode, owner, group);
        if (error) {
            closedir(directory);
            return error;
        }
    }
    closedir(directory);
    return 0;
}

static int prepare_pathf(const char *format, int user, mode_t mode,
                         uid_t uid, gid_t gid) {
    char path[PATH_MAX];
    if (snprintf(path, sizeof(path), format, user) >= (int)sizeof(path))
        return ENAMETOOLONG;
    return ensure_dir(path, mode, uid, gid);
}

static int prepare_de(int user) {
    int error = prepare_pathf("/data/user_de/%d", user, 0771, AID_SYSTEM, AID_SYSTEM);
    if (error) return error;
    error = prepare_pathf("/data/misc_de/%d", user, 0771, AID_SYSTEM, AID_SYSTEM);
    if (error) return error;
    const char *roots[] = { "/data/misc_de", "/data/misc" };
    for (size_t i = 0; i < sizeof(roots) / sizeof(roots[0]); ++i) {
        char path[PATH_MAX], pattern[PATH_MAX];
        snprintf(pattern, sizeof(pattern), "%s/%%d", roots[i]);
        if (snprintf(path, sizeof(path), pattern, user) >= (int)sizeof(path))
            return ENAMETOOLONG;
        const char *children[][2] = {
            { "vold", "0700" }, { "storaged", "0700" }, { "rollback", "0700" },
            { "apexrollback", "0700" }, { "sdksandbox", "0771" },
        };
        for (size_t j = 0; j < sizeof(children) / sizeof(children[0]); ++j) {
            char child[PATH_MAX];
            snprintf(child, sizeof(child), "%s/%s", path, children[j][0]);
            error = ensure_dir(child,
                strcmp(children[j][1], "0700") == 0 ? 0700 : 0771,
                strcmp(children[j][1], "0700") == 0 ? AID_ROOT : AID_SYSTEM,
                strcmp(children[j][1], "0700") == 0 ? AID_ROOT : AID_SYSTEM);
            if (error) return error;
        }
        error = prepare_apex_data(path, 0771, AID_ROOT, AID_SYSTEM);
        if (error) return error;
    }
    error = prepare_pathf("/data/misc/profiles/cur/%d", user, 0771,
                          AID_SYSTEM, AID_SYSTEM);
    if (error) return error;
    error = prepare_pathf("/data/vendor_de/%d/fpdata", user, 0700,
                          AID_SYSTEM, AID_SYSTEM);
    if (error) return error;
    error = prepare_pathf("/data/vendor_de/%d/facedata", user, 0700,
                          AID_SYSTEM, AID_SYSTEM);
    if (error) return error;
    // Scarlet currently uses vold's credential-key emulation path. fscrypt
    // unlocks the CE roots even while only DE storage is being initialized.
    error = prepare_pathf("/data/system_ce/%d", user, 0771,
                          AID_SYSTEM, AID_SYSTEM);
    if (error) return error;
    error = prepare_pathf("/data/misc_ce/%d", user, 01771,
                          AID_SYSTEM, AID_SYSTEM);
    if (error) return error;
    error = prepare_pathf("/data/media/%d", user, 0770,
                          AID_SYSTEM, AID_SYSTEM);
    if (error) return error;
    return prepare_pathf("/data/user/%d", user, 0771, AID_SYSTEM, AID_SYSTEM);
}

static int prepare_ce(int user) {
    struct stat st;
    int error;
    if (lstat("/data/data", &st) == 0 && S_ISDIR(st.st_mode)) {
        error = ensure_dir("/data/data", 0771, AID_SYSTEM, AID_SYSTEM);
    } else {
        error = prepare_pathf("/data/user/%d", user, 0771, AID_SYSTEM, AID_SYSTEM);
    }
    if (error) return error;
    error = prepare_pathf("/data/misc_ce/%d", user, 0771, AID_SYSTEM, AID_SYSTEM);
    if (error) return error;
    const char *roots[] = { "/data/misc_ce", "/data/misc" };
    for (size_t i = 0; i < sizeof(roots) / sizeof(roots[0]); ++i) {
        char path[PATH_MAX], pattern[PATH_MAX];
        snprintf(pattern, sizeof(pattern), "%s/%%d", roots[i]);
        if (snprintf(path, sizeof(path), pattern, user) >= (int)sizeof(path))
            return ENAMETOOLONG;
        const char *children[][2] = {
            { "vold", "0700" }, { "storaged", "0700" }, { "rollback", "0700" },
            { "apexrollback", "0700" }, { "sdksandbox", "0771" },
        };
        for (size_t j = 0; j < sizeof(children) / sizeof(children[0]); ++j) {
            char child[PATH_MAX];
            snprintf(child, sizeof(child), "%s/%s", path, children[j][0]);
            error = ensure_dir(child,
                strcmp(children[j][1], "0700") == 0 ? 0700 : 0771,
                strcmp(children[j][1], "0700") == 0 ? AID_ROOT : AID_SYSTEM,
                strcmp(children[j][1], "0700") == 0 ? AID_ROOT : AID_SYSTEM);
            if (error) return error;
        }
        char child[PATH_MAX];
        snprintf(child, sizeof(child), "%s/checkin", path);
        error = ensure_dir(child, 0770, AID_SYSTEM, AID_CACHE);
        if (error) return error;
        error = prepare_apex_data(path, 0771, AID_ROOT, AID_SYSTEM);
        if (error) return error;
    }
    error = prepare_pathf("/data/system_ce/%d/backup", user, 0700,
                          AID_SYSTEM, AID_SYSTEM);
    if (error) return error;
    error = prepare_pathf("/data/system_ce/%d/backup_stage", user, 0700,
                          AID_SYSTEM, AID_SYSTEM);
    if (error) return error;
    return prepare_pathf("/data/vendor_ce/%d/facedata", user, 0700,
                         AID_SYSTEM, AID_SYSTEM);
}

int main(int argc, char **argv) {
    if (argc != 5) {
        fprintf(stderr, "usage: %s prepare <uuid> <user_id> <flags>\n", argv[0]);
        return EINVAL;
    }
    char *end = NULL;
    long user = strtol(argv[3], &end, 10);
    if (!*argv[3] || *end || user < 0 || user > 21474) return EINVAL;
    long flags = strtol(argv[4], &end, 10);
    if (!*argv[4] || *end || (flags & ~(STORAGE_FLAG_DE | STORAGE_FLAG_CE))) return EINVAL;
    if (strcmp(argv[1], "prepare") != 0) {
        // The outer Android manager owns user removal and volume cleanup.
        return strcmp(argv[1], "destroy") == 0 ? 0 : EINVAL;
    }
    int error = 0;
    if (flags & STORAGE_FLAG_DE) error = prepare_de((int)user);
    if (!error && flags & STORAGE_FLAG_CE) error = prepare_ce((int)user);
    if (error) fprintf(stderr, "vold_prepare_subdirs: %s\n", strerror(error));
    return error;
}
