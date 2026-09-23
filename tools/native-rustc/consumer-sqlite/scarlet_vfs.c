/* Native SQLite VFS. SQLite itself remains the unmodified upstream release.
 * All SQLite locks use a real exclusive, nonblocking inode lock. This trades
 * concurrent readers for a small, safe rollback-journal implementation.
 */
#include <errno.h>
#include <fcntl.h>
#include <limits.h>
#include <pthread.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <sys/file.h>
#include <sys/random.h>
#include <sys/time.h>
#include <time.h>
#include <unistd.h>
#include "sqlite3.h"

#define SCARLET_PATH_MAX 1024

/* SQLITE_OS_OTHER selects SQLite's default no-op mutex backend even when
 * SQLITE_MUTEX_PTHREADS is predefined. Install the documented custom mutex
 * interface before initialization so the upstream source stays unmodified.
 * These mutexes also protect SQLite's allocator, PRNG, VFS registry and each
 * serialized connection, not merely the fixture's worker start gate. */
struct sqlite3_mutex {
    pthread_mutex_t native;
    int dynamic;
};

#define SCARLET_STATIC_MUTEX { PTHREAD_MUTEX_INITIALIZER, 0 }
static sqlite3_mutex scarlet_static_mutexes[] = {
    SCARLET_STATIC_MUTEX, SCARLET_STATIC_MUTEX, SCARLET_STATIC_MUTEX,
    SCARLET_STATIC_MUTEX, SCARLET_STATIC_MUTEX, SCARLET_STATIC_MUTEX,
    SCARLET_STATIC_MUTEX, SCARLET_STATIC_MUTEX, SCARLET_STATIC_MUTEX,
    SCARLET_STATIC_MUTEX, SCARLET_STATIC_MUTEX, SCARLET_STATIC_MUTEX,
};
#undef SCARLET_STATIC_MUTEX

static int scarlet_mutex_init(void) { return SQLITE_OK; }
static int scarlet_mutex_end(void) { return SQLITE_OK; }

static sqlite3_mutex *scarlet_mutex_alloc(int kind) {
    sqlite3_mutex *mutex;
    pthread_mutexattr_t attribute;
    int result;
    if (kind >= SQLITE_MUTEX_STATIC_MAIN && kind <= SQLITE_MUTEX_STATIC_VFS3)
        return &scarlet_static_mutexes[kind - SQLITE_MUTEX_STATIC_MAIN];
    if (kind != SQLITE_MUTEX_FAST && kind != SQLITE_MUTEX_RECURSIVE) return NULL;
    mutex = malloc(sizeof(*mutex));
    if (mutex == NULL) return NULL;
    mutex->dynamic = 1;
    if (kind == SQLITE_MUTEX_RECURSIVE) {
        if (pthread_mutexattr_init(&attribute) != 0) {
            free(mutex);
            return NULL;
        }
        result = pthread_mutexattr_settype(&attribute, PTHREAD_MUTEX_RECURSIVE);
        if (result == 0) result = pthread_mutex_init(&mutex->native, &attribute);
        if (pthread_mutexattr_destroy(&attribute) != 0) abort();
    } else {
        result = pthread_mutex_init(&mutex->native, NULL);
    }
    if (result != 0) {
        free(mutex);
        return NULL;
    }
    return mutex;
}

static void scarlet_mutex_free(sqlite3_mutex *mutex) {
    if (!mutex->dynamic || pthread_mutex_destroy(&mutex->native) != 0) abort();
    free(mutex);
}

static void scarlet_mutex_enter(sqlite3_mutex *mutex) {
    /* SQLite's void mutex methods cannot propagate a pthread error. A broken
     * mutex must terminate rather than silently enter an unprotected region. */
    if (pthread_mutex_lock(&mutex->native) != 0) abort();
}

static int scarlet_mutex_try(sqlite3_mutex *mutex) {
    int result = pthread_mutex_trylock(&mutex->native);
    if (result == 0) return SQLITE_OK;
    if (result == EBUSY) return SQLITE_BUSY;
    abort();
}

static void scarlet_mutex_leave(sqlite3_mutex *mutex) {
    if (pthread_mutex_unlock(&mutex->native) != 0) abort();
}

int scarlet_sqlite_configure(void) {
    static const sqlite3_mutex_methods methods = {
        scarlet_mutex_init, scarlet_mutex_end, scarlet_mutex_alloc,
        scarlet_mutex_free, scarlet_mutex_enter, scarlet_mutex_try,
        scarlet_mutex_leave, NULL, NULL,
    };
    int result = sqlite3_config(SQLITE_CONFIG_SERIALIZED);
    if (result == SQLITE_OK) result = sqlite3_config(SQLITE_CONFIG_MUTEX, &methods);
    return result;
}

typedef struct ScarletFile {
    sqlite3_file base;
    int fd;
    int lock_level;
    int sync_parent;
    const char *name;
} ScarletFile;

/* File state belongs to one SQLite handle and is protected by that
 * connection's serialized SQLite mutex. Separate handles share no mutable
 * user-space state; inode flock arbitrates them in the kernel. VFS callbacks
 * for time/randomness use caller-owned buffers and stack locals, and errno
 * comes from libc's per-thread storage. Initialization and shutdown happen
 * outside worker lifetimes. */

static int busy_errno(void) {
    return errno == EAGAIN || errno == EWOULDBLOCK;
}

static int lock_fd(int fd, int operation) {
    int result;
    do { result = flock(fd, operation); } while (result < 0 && errno == EINTR);
    return result;
}

static int scarlet_close(sqlite3_file *file) {
    ScarletFile *native = (ScarletFile *)file;
    int result = close(native->fd);
    native->fd = -1;
    native->lock_level = SQLITE_LOCK_NONE;
    native->base.pMethods = NULL;
    /* Closing the last reference releases its real kernel lock. Do not retry
     * close on EINTR: some implementations have already consumed the fd. */
    return result == 0 ? SQLITE_OK : SQLITE_IOERR_CLOSE;
}

static int scarlet_read(sqlite3_file *file, void *buffer, int amount,
                        sqlite3_int64 offset) {
    ScarletFile *native = (ScarletFile *)file;
    unsigned char *bytes = buffer;
    int done = 0;
    if (amount < 0 || offset < 0 || offset > INT64_MAX - amount)
        return SQLITE_IOERR_READ;
    while (done < amount) {
        ssize_t count = pread(native->fd, bytes + done, (size_t)(amount - done),
                              (off_t)(offset + done));
        if (count < 0 && errno == EINTR) continue;
        if (count < 0) return SQLITE_IOERR_READ;
        if (count == 0) {
            /* Required by SQLite, including reads beyond the whole file. */
            memset(bytes + done, 0, (size_t)(amount - done));
            return SQLITE_IOERR_SHORT_READ;
        }
        if (count > amount - done) return SQLITE_IOERR_READ;
        done += (int)count;
    }
    return SQLITE_OK;
}

static int scarlet_write(sqlite3_file *file, const void *buffer, int amount,
                         sqlite3_int64 offset) {
    ScarletFile *native = (ScarletFile *)file;
    const unsigned char *bytes = buffer;
    int done = 0;
    if (amount < 0 || offset < 0 || offset > INT64_MAX - amount)
        return SQLITE_IOERR_WRITE;
    while (done < amount) {
        ssize_t count = pwrite(native->fd, bytes + done,
                               (size_t)(amount - done), (off_t)(offset + done));
        if (count < 0 && errno == EINTR) continue;
        if (count < 0) return errno == ENOSPC ? SQLITE_FULL : SQLITE_IOERR_WRITE;
        if (count == 0 || count > amount - done) return SQLITE_IOERR_WRITE;
        done += (int)count;
    }
    return SQLITE_OK;
}

static int scarlet_truncate(sqlite3_file *file, sqlite3_int64 size) {
    ScarletFile *native = (ScarletFile *)file;
    int result;
    if (size < 0) return SQLITE_IOERR_TRUNCATE;
    do { result = ftruncate(native->fd, (off_t)size); }
    while (result < 0 && errno == EINTR);
    return result == 0 ? SQLITE_OK : SQLITE_IOERR_TRUNCATE;
}

static int sync_fd(int fd, int data_only) {
    int result;
    do { result = data_only ? fdatasync(fd) : fsync(fd); }
    while (result < 0 && errno == EINTR);
    return result;
}

static int sync_parent(const char *name);

static int scarlet_sync(sqlite3_file *file, int flags) {
    ScarletFile *native = (ScarletFile *)file;
    if (sync_fd(native->fd, flags & SQLITE_SYNC_DATAONLY) != 0)
        return SQLITE_IOERR_FSYNC;
    if (native->sync_parent) {
        if (sync_parent(native->name) != SQLITE_OK) return SQLITE_IOERR_DIR_FSYNC;
        native->sync_parent = 0;
    }
    return SQLITE_OK;
}

static int scarlet_size(sqlite3_file *file, sqlite3_int64 *size) {
    ScarletFile *native = (ScarletFile *)file;
    off_t end = lseek(native->fd, 0, SEEK_END);
    if (end < 0) return SQLITE_IOERR_FSTAT;
    /* All data I/O is positioned; the descriptor cursor is not used. */
    *size = (sqlite3_int64)end;
    return SQLITE_OK;
}

static int scarlet_lock(sqlite3_file *file, int level) {
    ScarletFile *native = (ScarletFile *)file;
    if (level < SQLITE_LOCK_SHARED || level > SQLITE_LOCK_EXCLUSIVE)
        return SQLITE_IOERR_LOCK;
    if (level <= native->lock_level) return SQLITE_OK;
    if (native->lock_level == SQLITE_LOCK_NONE
        && lock_fd(native->fd, LOCK_EX | LOCK_NB) != 0)
        return busy_errno() ? SQLITE_BUSY : SQLITE_IOERR_LOCK;
    /* An already held physical lock is exclusive, even when SQLite's logical
     * level is SHARED. Only our own real lock permits this logical upgrade. */
    native->lock_level = level;
    return SQLITE_OK;
}

static int scarlet_unlock(sqlite3_file *file, int level) {
    ScarletFile *native = (ScarletFile *)file;
    if (level != SQLITE_LOCK_NONE && level != SQLITE_LOCK_SHARED)
        return SQLITE_IOERR_UNLOCK;
    if (level >= native->lock_level) return SQLITE_OK;
    if (level == SQLITE_LOCK_NONE && lock_fd(native->fd, LOCK_UN) != 0)
        return SQLITE_IOERR_UNLOCK;
    native->lock_level = level;
    return SQLITE_OK;
}

static int scarlet_reserved(sqlite3_file *file, int *reserved) {
    ScarletFile *native = (ScarletFile *)file;
    *reserved = 0;
    if (native->lock_level != SQLITE_LOCK_NONE) {
        /* SHARED cannot be reported as RESERVED, or hot-journal recovery
         * would incorrectly skip rollback while we hold our physical lock. */
        *reserved = native->lock_level >= SQLITE_LOCK_RESERVED;
        return SQLITE_OK;
    }
    if (lock_fd(native->fd, LOCK_EX | LOCK_NB) != 0) {
        if (!busy_errno()) return SQLITE_IOERR_CHECKRESERVEDLOCK;
        *reserved = 1; /* Another owner holds a conservatively exclusive lock. */
        return SQLITE_OK;
    }
    return lock_fd(native->fd, LOCK_UN) == 0
        ? SQLITE_OK : SQLITE_IOERR_CHECKRESERVEDLOCK;
}

static int scarlet_control(sqlite3_file *file, int operation, void *argument) {
    ScarletFile *native = (ScarletFile *)file;
    if (operation == SQLITE_FCNTL_LOCKSTATE) {
        *(int *)argument = native->lock_level;
        return SQLITE_OK;
    }
    return SQLITE_NOTFOUND;
}

static int scarlet_sector(sqlite3_file *file) {
    (void)file;
    /* Conservative sector bound for the currently tested virtio/ext2 stack. */
    return 4096;
}

static int scarlet_characteristics(sqlite3_file *file) {
    (void)file;
    return 0; /* No unverified atomic-write or powersafe-overwrite promises. */
}

static const sqlite3_io_methods scarlet_methods = {
    .iVersion = 1,
    .xClose = scarlet_close,
    .xRead = scarlet_read,
    .xWrite = scarlet_write,
    .xTruncate = scarlet_truncate,
    .xSync = scarlet_sync,
    .xFileSize = scarlet_size,
    .xLock = scarlet_lock,
    .xUnlock = scarlet_unlock,
    .xCheckReservedLock = scarlet_reserved,
    .xFileControl = scarlet_control,
    .xSectorSize = scarlet_sector,
    .xDeviceCharacteristics = scarlet_characteristics,
};

static int scarlet_open(sqlite3_vfs *vfs, const char *name, sqlite3_file *file,
                        int flags, int *output_flags) {
    ScarletFile *native = (ScarletFile *)file;
    int mode = (flags & SQLITE_OPEN_READONLY) ? O_RDONLY : O_RDWR;
    int fd;
    (void)vfs;
    memset(native, 0, sizeof(*native));
    native->fd = -1;
    /* Memory temp storage is explicit in this build. Never pretend a disk
     * temp file or WAL/shared-memory file has been opened successfully. */
    if (name == NULL || (flags & (SQLITE_OPEN_NOFOLLOW
        | SQLITE_OPEN_DELETEONCLOSE | SQLITE_OPEN_WAL
        | SQLITE_OPEN_TEMP_DB | SQLITE_OPEN_TEMP_JOURNAL
        | SQLITE_OPEN_TRANSIENT_DB | SQLITE_OPEN_SUBJOURNAL)))
        return SQLITE_CANTOPEN;
    if (!(flags & (SQLITE_OPEN_MAIN_DB | SQLITE_OPEN_MAIN_JOURNAL
                    | SQLITE_OPEN_SUPER_JOURNAL))) return SQLITE_CANTOPEN;
    if (flags & SQLITE_OPEN_CREATE) mode |= O_CREAT;
    if (flags & SQLITE_OPEN_EXCLUSIVE) mode |= O_EXCL;
    /* xFullPathname resolves existing aliases. Refuse a dangling or changed
     * final symlink rather than create a different database/journal pair. */
    mode |= O_CLOEXEC | O_NOFOLLOW;
    do { fd = open(name, mode, 0600); } while (fd < 0 && errno == EINTR);
    if (fd < 0 && (flags & SQLITE_OPEN_MAIN_DB)
        && !(flags & SQLITE_OPEN_EXCLUSIVE) && (flags & SQLITE_OPEN_READWRITE)
        && (errno == EACCES || errno == EROFS)) {
        do { fd = open(name, O_RDONLY | O_CLOEXEC | O_NOFOLLOW); }
        while (fd < 0 && errno == EINTR);
        if (fd >= 0) flags = (flags & ~SQLITE_OPEN_READWRITE) | SQLITE_OPEN_READONLY;
    }
    if (fd < 0) return SQLITE_CANTOPEN;
    native->fd = fd;
    native->name = name; /* SQLite guarantees this storage until xClose. */
    native->sync_parent = (flags & SQLITE_OPEN_CREATE) != 0
        && (flags & (SQLITE_OPEN_MAIN_JOURNAL | SQLITE_OPEN_SUPER_JOURNAL)) != 0;
    native->base.pMethods = &scarlet_methods;
    if (output_flags != NULL) *output_flags = flags;
    return SQLITE_OK;
}

static int parent_path(const char *name, char *parent) {
    const char *slash = strrchr(name, '/');
    size_t length = slash == NULL ? 0u : (size_t)(slash - name);
    if (length >= SCARLET_PATH_MAX) return SQLITE_CANTOPEN;
    if (slash == NULL) {
        parent[0] = '.';
        length = 1;
    } else if (length == 0) {
        parent[0] = '/';
        length = 1;
    } else {
        memcpy(parent, name, length);
    }
    parent[length] = '\0';
    return SQLITE_OK;
}

static int sync_parent(const char *name) {
    char parent[SCARLET_PATH_MAX];
    int result;
    int fd;
    if (parent_path(name, parent) != SQLITE_OK) return SQLITE_IOERR_DIR_FSYNC;
    do { fd = open(parent, O_RDONLY | O_DIRECTORY | O_CLOEXEC); }
    while (fd < 0 && errno == EINTR);
    if (fd < 0) return SQLITE_IOERR_DIR_FSYNC;
    result = sync_fd(fd, 0);
    if (close(fd) != 0) result = -1;
    return result == 0 ? SQLITE_OK : SQLITE_IOERR_DIR_FSYNC;
}

static int scarlet_delete(sqlite3_vfs *vfs, const char *name, int sync_directory) {
    int result;
    (void)vfs;
    do { result = unlink(name); } while (result < 0 && errno == EINTR);
    if (result < 0) return errno == ENOENT
        ? SQLITE_IOERR_DELETE_NOENT : SQLITE_IOERR_DELETE;
    return (sync_directory & 1) ? sync_parent(name) : SQLITE_OK;
}

static int scarlet_access(sqlite3_vfs *vfs, const char *name, int flags,
                          int *accessible) {
    int fd;
    int mode;
    (void)vfs;
    *accessible = 0;
    if (flags != SQLITE_ACCESS_EXISTS && flags != SQLITE_ACCESS_READ
        && flags != SQLITE_ACCESS_READWRITE) return SQLITE_IOERR_ACCESS;
    mode = flags == SQLITE_ACCESS_READWRITE ? O_RDWR : O_RDONLY;
    do { fd = open(name, mode | O_CLOEXEC | O_NOFOLLOW); }
    while (fd < 0 && errno == EINTR);
    if (fd < 0) {
        if (errno == ENOENT || errno == ENOTDIR) return SQLITE_OK;
        /* An unreadable existing journal must not be reported absent: doing
         * so could skip hot-journal recovery. */
        if (flags != SQLITE_ACCESS_EXISTS && (errno == EACCES || errno == EROFS))
            return SQLITE_OK;
        return SQLITE_IOERR_ACCESS;
    }
    *accessible = 1;
    return close(fd) == 0 ? SQLITE_OK : SQLITE_IOERR_ACCESS;
}

static int scarlet_full_path(sqlite3_vfs *vfs, const char *name, int output_size,
                             char *output) {
    char parent[SCARLET_PATH_MAX];
    char *resolved;
    const char *leaf;
    size_t length;
    (void)vfs;
    if (name == NULL || name[0] == '\0' || output_size <= 0)
        return SQLITE_CANTOPEN;
    resolved = realpath(name, NULL);
    if (resolved != NULL) {
        length = strlen(resolved);
        if (length >= (size_t)output_size || length >= SCARLET_PATH_MAX) {
            free(resolved);
            return SQLITE_CANTOPEN;
        }
        memcpy(output, resolved, length + 1);
        free(resolved);
        return SQLITE_OK;
    }
    if (errno != ENOENT || parent_path(name, parent) != SQLITE_OK)
        return SQLITE_CANTOPEN;
    leaf = strrchr(name, '/');
    leaf = leaf == NULL ? name : leaf + 1;
    if (leaf[0] == '\0' || strcmp(leaf, ".") == 0 || strcmp(leaf, "..") == 0)
        return SQLITE_CANTOPEN;
    resolved = realpath(parent, NULL);
    if (resolved == NULL) return SQLITE_CANTOPEN;
    length = strlen(resolved);
    if (length != 1 || resolved[0] != '/') length++;
    if (strlen(leaf) >= SCARLET_PATH_MAX - length
        || length + strlen(leaf) >= (size_t)output_size) {
        free(resolved);
        return SQLITE_CANTOPEN;
    }
    memcpy(output, resolved, strlen(resolved));
    if (length > strlen(resolved)) output[length - 1] = '/';
    memcpy(output + length, leaf, strlen(leaf) + 1);
    free(resolved);
    return SQLITE_OK;
}

static int scarlet_random(sqlite3_vfs *vfs, int amount, char *buffer) {
    int done = 0;
    (void)vfs;
    if (amount <= 0) return 0;
    memset(buffer, 0, (size_t)amount);
    while (done < amount) {
        ssize_t count = getrandom(buffer + done, (size_t)(amount - done), 0);
        if (count < 0 && errno == EINTR) continue;
        if (count <= 0 || count > amount - done) break;
        done += (int)count;
    }
    return done;
}

static int scarlet_sleep(sqlite3_vfs *vfs, int microseconds) {
    struct timespec request;
    struct timespec remaining;
    (void)vfs;
    if (microseconds <= 0) return 0;
    request.tv_sec = microseconds / 1000000;
    request.tv_nsec = (microseconds % 1000000) * 1000L;
    while (nanosleep(&request, &remaining) != 0) {
        if (errno != EINTR) return 0;
        request = remaining;
    }
    return microseconds;
}

static int scarlet_time64(sqlite3_vfs *vfs, sqlite3_int64 *milliseconds) {
    struct timeval now;
    const sqlite3_int64 epoch = 210866760000000LL;
    (void)vfs;
    if (gettimeofday(&now, NULL) != 0 || now.tv_sec < 0
        || now.tv_sec > (INT64_MAX - epoch - 999) / 1000
        || now.tv_usec < 0 || now.tv_usec >= 1000000) return SQLITE_ERROR;
    *milliseconds = epoch + (sqlite3_int64)now.tv_sec * 1000 + now.tv_usec / 1000;
    return SQLITE_OK;
}

static int scarlet_time(sqlite3_vfs *vfs, double *days) {
    sqlite3_int64 milliseconds;
    int result = scarlet_time64(vfs, &milliseconds);
    if (result == SQLITE_OK) *days = milliseconds / 86400000.0;
    return result;
}

static int scarlet_error(sqlite3_vfs *vfs, int size, char *message) {
    int saved_errno = errno;
    (void)vfs;
    if (size > 0) sqlite3_snprintf(size, message, "%s", strerror(saved_errno));
    return saved_errno;
}

static sqlite3_vfs scarlet_vfs = {
    .iVersion = 2, /* Version 2 only adds the integer clock, not WAL methods. */
    .szOsFile = sizeof(ScarletFile),
    .mxPathname = SCARLET_PATH_MAX - 1,
    .zName = "scarlet-native",
    .xOpen = scarlet_open,
    .xDelete = scarlet_delete,
    .xAccess = scarlet_access,
    .xFullPathname = scarlet_full_path,
    .xRandomness = scarlet_random,
    .xSleep = scarlet_sleep,
    .xCurrentTime = scarlet_time,
    .xGetLastError = scarlet_error,
    .xCurrentTimeInt64 = scarlet_time64,
};

int sqlite3_os_init(void) {
    return sqlite3_vfs_register(&scarlet_vfs, 1);
}

int sqlite3_os_end(void) {
    return sqlite3_vfs_unregister(&scarlet_vfs);
}
