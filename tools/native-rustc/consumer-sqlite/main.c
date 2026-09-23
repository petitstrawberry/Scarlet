/* Real SQLite SQL/storage acceptance against the Scarlet-native VFS. */
#include <errno.h>
#include <fcntl.h>
#include <pthread.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include "sqlite3.h"

int scarlet_sqlite_configure(void);

#define BLOB_SIZE 131113
#define CHANGE_OFFSET 65539
#define CHANGE_SIZE 4099
#define CHECK(expression) do { \
    if (!(expression)) { \
        printf("SCARLET_SQLITE FAIL line=%d expression=%s errno=%d\n", \
               __LINE__, #expression, errno); \
        return 2; \
    } \
} while (0)
#define SQL(database, text) do { \
    int sql_result = execute((database), (text), __LINE__); \
    CHECK(sql_result == SQLITE_OK); \
} while (0)
#define DB_CHECK(database, expression) do { \
    if (!(expression)) { \
        printf("SCARLET_SQLITE FAIL line=%d expression=%s sqlite=%d message=%s errno=%d\n", \
               __LINE__, #expression, sqlite3_extended_errcode(database), \
               sqlite3_errmsg(database), errno); \
        return 2; \
    } \
} while (0)

static unsigned char blob[BLOB_SIZE];
static unsigned char readback[BLOB_SIZE + 32];

/* A synchronization failure cannot safely fall through a gate or leave a
 * worker accessing a returned stack frame. Terminate this failed fixture. */
#define THREAD_CALL(expression) do { \
    int thread_result = (expression); \
    if (thread_result != 0) { \
        printf("SCARLET_SQLITE PTHREAD FAIL line=%d expression=%s result=%d\n", \
               __LINE__, #expression, thread_result); \
        abort(); \
    } \
} while (0)

typedef struct ThreadGate {
    pthread_mutex_t mutex;
    pthread_cond_t condition;
    int ready;
    int phase;
} ThreadGate;

#define THREAD_GATE_INITIALIZER { PTHREAD_MUTEX_INITIALIZER, PTHREAD_COND_INITIALIZER, 0, 0 }

static void worker_gate(ThreadGate *gate) {
    THREAD_CALL(pthread_mutex_lock(&gate->mutex));
    gate->ready++;
    THREAD_CALL(pthread_cond_broadcast(&gate->condition));
    while (gate->phase == 0)
        THREAD_CALL(pthread_cond_wait(&gate->condition, &gate->mutex));
    THREAD_CALL(pthread_mutex_unlock(&gate->mutex));
}

static void gate_wait(ThreadGate *gate, int count) {
    THREAD_CALL(pthread_mutex_lock(&gate->mutex));
    while (gate->ready < count)
        THREAD_CALL(pthread_cond_wait(&gate->condition, &gate->mutex));
    THREAD_CALL(pthread_mutex_unlock(&gate->mutex));
}

static void gate_release(ThreadGate *gate) {
    THREAD_CALL(pthread_mutex_lock(&gate->mutex));
    gate->phase = 1;
    THREAD_CALL(pthread_cond_broadcast(&gate->condition));
    THREAD_CALL(pthread_mutex_unlock(&gate->mutex));
}

static void gate_destroy(ThreadGate *gate) {
    THREAD_CALL(pthread_cond_destroy(&gate->condition));
    THREAD_CALL(pthread_mutex_destroy(&gate->mutex));
}

static int execute(sqlite3 *database, const char *sql, int line) {
    char *message = NULL;
    int result = sqlite3_exec(database, sql, NULL, NULL, &message);
    if (result != SQLITE_OK) {
        printf("SCARLET_SQLITE SQL line=%d rc=%d sql=%s message=%s\n",
               line, result, sql, message == NULL ? sqlite3_errmsg(database) : message);
    }
    sqlite3_free(message);
    return result;
}

static int make_path(char *path, size_t size, const char *directory,
                     const char *name) {
    int length = snprintf(path, size, "%s/%s", directory, name);
    CHECK(length > 0 && (size_t)length < size);
    return 0;
}

static void make_blob(int changed) {
    unsigned int state = 0x735c91a7u;
    int index;
    for (index = 0; index < BLOB_SIZE; ++index) {
        state = state * 1664525u + 1013904223u;
        blob[index] = (unsigned char)((state >> 24) ^ (unsigned int)index);
        if (changed && index >= CHANGE_OFFSET && index < CHANGE_OFFSET + CHANGE_SIZE)
            blob[index] ^= 0x5a;
    }
}

static int scalar_integer(sqlite3 *database, const char *sql,
                          sqlite3_int64 expected) {
    sqlite3_stmt *statement = NULL;
    DB_CHECK(database, sqlite3_prepare_v2(database, sql, -1, &statement, NULL) == SQLITE_OK);
    DB_CHECK(database, sqlite3_step(statement) == SQLITE_ROW);
    DB_CHECK(database, sqlite3_column_int64(statement, 0) == expected);
    DB_CHECK(database, sqlite3_step(statement) == SQLITE_DONE);
    DB_CHECK(database, sqlite3_finalize(statement) == SQLITE_OK);
    return 0;
}

static int scalar_text(sqlite3 *database, const char *sql, const char *expected) {
    sqlite3_stmt *statement = NULL;
    const unsigned char *value;
    DB_CHECK(database, sqlite3_prepare_v2(database, sql, -1, &statement, NULL) == SQLITE_OK);
    DB_CHECK(database, sqlite3_step(statement) == SQLITE_ROW);
    value = sqlite3_column_text(statement, 0);
    DB_CHECK(database, value != NULL && strcmp((const char *)value, expected) == 0);
    DB_CHECK(database, sqlite3_step(statement) == SQLITE_DONE);
    DB_CHECK(database, sqlite3_finalize(statement) == SQLITE_OK);
    return 0;
}

static int verify_rows(sqlite3 *database) {
    sqlite3_stmt *statement = NULL;
    int row;
    CHECK(scalar_integer(database, "SELECT count(*) FROM items", 24) == 0);
    CHECK(scalar_integer(database, "SELECT count(*) FROM groups", 3) == 0);
    CHECK(scalar_integer(database, "SELECT count(*) FROM items WHERE id >= 99", 0) == 0);
    CHECK(scalar_text(database, "PRAGMA integrity_check", "ok") == 0);
    CHECK(scalar_text(database, "SELECT typeof(sum(score)) FROM items", "real") == 0);
    DB_CHECK(database, sqlite3_prepare_v2(database,
        "SELECT i.id,i.label,i.score,i.group_id,i.payload,g.title "
        "FROM items i JOIN groups g ON g.id=i.group_id ORDER BY i.id",
        -1, &statement, NULL) == SQLITE_OK);
    make_blob(1);
    for (row = 1; row <= 24; ++row) {
        char label[64];
        char group[32];
        const unsigned char *text;
        CHECK(snprintf(label, sizeof(label), "scarlet-%02d-苺", row) > 0);
        CHECK(snprintf(group, sizeof(group), "group-%d", row % 3) > 0);
        DB_CHECK(database, sqlite3_step(statement) == SQLITE_ROW);
        DB_CHECK(database, sqlite3_column_int(statement, 0) == row);
        text = sqlite3_column_text(statement, 1);
        DB_CHECK(database, text != NULL && strcmp((const char *)text, label) == 0);
        DB_CHECK(database, sqlite3_column_type(statement, 2) == SQLITE_FLOAT);
        DB_CHECK(database, sqlite3_column_double(statement, 2) == row * 0.25);
        DB_CHECK(database, sqlite3_column_int(statement, 3) == row % 3);
        if (row == 7) {
            const void *payload = sqlite3_column_blob(statement, 4);
            DB_CHECK(database, payload != NULL && sqlite3_column_bytes(statement, 4) == BLOB_SIZE);
            DB_CHECK(database, memcmp(payload, blob, BLOB_SIZE) == 0);
        } else {
            DB_CHECK(database, sqlite3_column_type(statement, 4) == SQLITE_NULL);
        }
        text = sqlite3_column_text(statement, 5);
        DB_CHECK(database, text != NULL && strcmp((const char *)text, group) == 0);
    }
    DB_CHECK(database, sqlite3_step(statement) == SQLITE_DONE);
    DB_CHECK(database, sqlite3_finalize(statement) == SQLITE_OK);
    CHECK(scalar_integer(database,
        "SELECT count(*) FROM (SELECT group_id,count(*) AS n FROM items "
        "GROUP BY group_id HAVING n=8)", 3) == 0);
    CHECK(scalar_integer(database, "SELECT sum(score)=75.0 FROM items", 1) == 0);
    CHECK(scalar_integer(database, "SELECT length(randomblob(37))", 37) == 0);
    CHECK(scalar_integer(database, "SELECT unixepoch('now')>0", 1) == 0);
    DB_CHECK(database, sqlite3_prepare_v2(database,
        "EXPLAIN QUERY PLAN SELECT id FROM items WHERE group_id=1",
        -1, &statement, NULL) == SQLITE_OK);
    DB_CHECK(database, sqlite3_step(statement) == SQLITE_ROW);
    DB_CHECK(database, strstr((const char *)sqlite3_column_text(statement, 3),
                              "idx_items_group") != NULL);
    DB_CHECK(database, sqlite3_finalize(statement) == SQLITE_OK);
    return 0;
}

static int vfs_test(const char *directory) {
    sqlite3_vfs *vfs = sqlite3_vfs_find("scarlet-native");
    sqlite3_file *first;
    sqlite3_file *second;
    char path[1024];
    char canonical[1024];
    sqlite3_int64 size;
    int flags = 0;
    int value;
    int index;
    CHECK(vfs != NULL);
    CHECK(make_path(path, sizeof(path), directory, "sqlite-vfs-io.bin") == 0);
    CHECK(unlink(path) == 0 || errno == ENOENT);
    CHECK(vfs->xFullPathname(vfs, path, sizeof(canonical), canonical) == SQLITE_OK);
    CHECK(vfs->xFullPathname(vfs, path, 1, canonical) == SQLITE_CANTOPEN);
    CHECK(vfs->xFullPathname(vfs, path, sizeof(canonical), canonical) == SQLITE_OK);
    first = calloc(1, (size_t)vfs->szOsFile);
    second = calloc(1, (size_t)vfs->szOsFile);
    CHECK(first != NULL && second != NULL);
    CHECK(vfs->xOpen(vfs, canonical, first, SQLITE_OPEN_MAIN_DB | SQLITE_OPEN_CREATE
        | SQLITE_OPEN_EXCLUSIVE | SQLITE_OPEN_READWRITE, &flags) == SQLITE_OK);
    CHECK(first->pMethods != NULL && first->pMethods->iVersion == 1);
    CHECK(vfs->xOpen(vfs, canonical, second, SQLITE_OPEN_MAIN_DB | SQLITE_OPEN_CREATE
        | SQLITE_OPEN_EXCLUSIVE | SQLITE_OPEN_READWRITE, NULL) != SQLITE_OK);
    CHECK(second->pMethods == NULL);
    make_blob(0);
    CHECK(first->pMethods->xWrite(first, blob, BLOB_SIZE, 17) == SQLITE_OK);
    CHECK(first->pMethods->xFileSize(first, &size) == SQLITE_OK && size == BLOB_SIZE + 17);
    CHECK(first->pMethods->xRead(first, readback, BLOB_SIZE, 17) == SQLITE_OK);
    CHECK(memcmp(blob, readback, BLOB_SIZE) == 0);
    memset(readback, 0xa5, sizeof(readback));
    CHECK(first->pMethods->xRead(first, readback, 49, BLOB_SIZE) == SQLITE_IOERR_SHORT_READ);
    CHECK(memcmp(readback, blob + BLOB_SIZE - 17, 17) == 0);
    for (index = 17; index < 49; ++index) CHECK(readback[index] == 0);
    CHECK(first->pMethods->xTruncate(first, 13) == SQLITE_OK);
    CHECK(first->pMethods->xFileSize(first, &size) == SQLITE_OK && size == 13);
    memset(readback, 0xa5, 32);
    CHECK(first->pMethods->xRead(first, readback, 32, 100) == SQLITE_IOERR_SHORT_READ);
    for (index = 0; index < 32; ++index) CHECK(readback[index] == 0);
    CHECK(first->pMethods->xSync(first, SQLITE_SYNC_FULL) == SQLITE_OK);
    CHECK(first->pMethods->xFileControl(first, 0x123456, NULL) == SQLITE_NOTFOUND);
    CHECK(vfs->xOpen(vfs, canonical, second, SQLITE_OPEN_MAIN_DB | SQLITE_OPEN_READWRITE,
                      NULL) == SQLITE_OK);
    CHECK(first->pMethods->xLock(first, SQLITE_LOCK_SHARED) == SQLITE_OK);
    CHECK(first->pMethods->xCheckReservedLock(first, &value) == SQLITE_OK && value == 0);
    CHECK(second->pMethods->xLock(second, SQLITE_LOCK_SHARED) == SQLITE_BUSY);
    CHECK(second->pMethods->xCheckReservedLock(second, &value) == SQLITE_OK && value == 1);
    CHECK(first->pMethods->xLock(first, SQLITE_LOCK_RESERVED) == SQLITE_OK);
    CHECK(first->pMethods->xCheckReservedLock(first, &value) == SQLITE_OK && value == 1);
    CHECK(first->pMethods->xUnlock(first, SQLITE_LOCK_SHARED) == SQLITE_OK);
    CHECK(second->pMethods->xLock(second, SQLITE_LOCK_SHARED) == SQLITE_BUSY);
    CHECK(first->pMethods->xUnlock(first, SQLITE_LOCK_NONE) == SQLITE_OK);
    CHECK(second->pMethods->xLock(second, SQLITE_LOCK_SHARED) == SQLITE_OK);
    CHECK(second->pMethods->xClose(second) == SQLITE_OK); /* Close releases lock. */
    CHECK(first->pMethods->xLock(first, SQLITE_LOCK_SHARED) == SQLITE_OK);
    CHECK(first->pMethods->xClose(first) == SQLITE_OK);
    free(first);
    free(second);
    CHECK(vfs->xAccess(vfs, canonical, SQLITE_ACCESS_EXISTS, &value) == SQLITE_OK && value == 1);
    CHECK(vfs->xDelete(vfs, canonical, 1) == SQLITE_OK);
    CHECK(vfs->xAccess(vfs, canonical, SQLITE_ACCESS_EXISTS, &value) == SQLITE_OK && value == 0);
    CHECK(vfs->xSleep(vfs, 1) >= 1);
    CHECK(puts("SCARLET_SQLITE VFS PASS") >= 0);
    return 0;
}

static int create_database(const char *path) {
    sqlite3 *database = NULL;
    sqlite3 *other = NULL;
    sqlite3_stmt *statement = NULL;
    sqlite3_blob *handle = NULL;
    int row;
    CHECK(unlink(path) == 0 || errno == ENOENT);
    DB_CHECK(database, sqlite3_open_v2(path, &database,
        SQLITE_OPEN_READWRITE | SQLITE_OPEN_CREATE, "scarlet-native") == SQLITE_OK);
    SQL(database, "PRAGMA page_size=1024; PRAGMA cache_size=8; PRAGMA foreign_keys=ON;");
    CHECK(scalar_text(database, "PRAGMA journal_mode=DELETE", "delete") == 0);
    SQL(database, "PRAGMA synchronous=FULL;");
    SQL(database, "CREATE TABLE groups(id INTEGER PRIMARY KEY,title TEXT NOT NULL);"
        "CREATE TABLE items(id INTEGER PRIMARY KEY,label TEXT NOT NULL UNIQUE,"
        "score REAL NOT NULL,group_id INTEGER NOT NULL REFERENCES groups(id),payload BLOB);"
        "CREATE INDEX idx_items_group ON items(group_id);"
        "INSERT INTO groups VALUES(0,'group-0'),(1,'group-1'),(2,'group-2');"
        "BEGIN IMMEDIATE;");
    DB_CHECK(database, sqlite3_prepare_v2(database,
        "INSERT INTO items VALUES(?1,?2,?3,?4,?5)", -1, &statement, NULL) == SQLITE_OK);
    make_blob(0);
    for (row = 1; row <= 24; ++row) {
        char label[64];
        CHECK(snprintf(label, sizeof(label), "scarlet-%02d-苺", row) > 0);
        DB_CHECK(database, sqlite3_bind_int(statement, 1, row) == SQLITE_OK);
        DB_CHECK(database, sqlite3_bind_text(statement, 2, label, -1, SQLITE_TRANSIENT) == SQLITE_OK);
        DB_CHECK(database, sqlite3_bind_double(statement, 3, row * 0.25) == SQLITE_OK);
        DB_CHECK(database, sqlite3_bind_int(statement, 4, row % 3) == SQLITE_OK);
        DB_CHECK(database, (row == 7
            ? sqlite3_bind_blob(statement, 5, blob, BLOB_SIZE, SQLITE_STATIC)
            : sqlite3_bind_null(statement, 5)) == SQLITE_OK);
        DB_CHECK(database, sqlite3_step(statement) == SQLITE_DONE);
        DB_CHECK(database, sqlite3_reset(statement) == SQLITE_OK);
    }
    DB_CHECK(database, sqlite3_finalize(statement) == SQLITE_OK);
    SQL(database, "COMMIT;");
    DB_CHECK(database, sqlite3_blob_open(database, "main", "items", "payload", 7, 1, &handle) == SQLITE_OK);
    DB_CHECK(database, sqlite3_blob_bytes(handle) == BLOB_SIZE);
    make_blob(1);
    DB_CHECK(database, sqlite3_blob_write(handle, blob + CHANGE_OFFSET, CHANGE_SIZE, CHANGE_OFFSET) == SQLITE_OK);
    DB_CHECK(database, sqlite3_blob_read(handle, readback, CHANGE_SIZE, CHANGE_OFFSET) == SQLITE_OK);
    DB_CHECK(database, memcmp(readback, blob + CHANGE_OFFSET, CHANGE_SIZE) == 0);
    DB_CHECK(database, sqlite3_blob_close(handle) == SQLITE_OK);
    SQL(database, "BEGIN; UPDATE items SET score=999.0 WHERE id=1;"
        "INSERT INTO items VALUES(99,'rolled-back',1.5,0,NULL); ROLLBACK;"
        "SAVEPOINT outer_save; INSERT INTO items VALUES(100,'savepoint',2.5,1,NULL);"
        "ROLLBACK TO outer_save; RELEASE outer_save;");
    DB_CHECK(database, sqlite3_open_v2(path, &other, SQLITE_OPEN_READWRITE, "scarlet-native") == SQLITE_OK);
    SQL(database, "BEGIN IMMEDIATE; UPDATE items SET score=123.0 WHERE id=2;");
    CHECK(sqlite3_exec(other, "SELECT count(*) FROM items", NULL, NULL, NULL) == SQLITE_BUSY);
    CHECK(sqlite3_exec(other, "BEGIN IMMEDIATE", NULL, NULL, NULL) == SQLITE_BUSY);
    SQL(database, "ROLLBACK;");
    CHECK(scalar_integer(other, "SELECT count(*) FROM items", 24) == 0);
    SQL(other, "BEGIN IMMEDIATE; UPDATE items SET score=456.0 WHERE id=2; ROLLBACK;");
    DB_CHECK(other, sqlite3_close(other) == SQLITE_OK);
    SQL(database, "VACUUM;");
    CHECK(verify_rows(database) == 0);
    DB_CHECK(database, sqlite3_close(database) == SQLITE_OK);
    CHECK(puts("SCARLET_SQLITE TRANSACTIONS PASS") >= 0);
    return 0;
}

static int reopen_database(const char *path) {
    sqlite3 *database = NULL;
    DB_CHECK(database, sqlite3_open_v2(path, &database, SQLITE_OPEN_READWRITE,
                                      "scarlet-native") == SQLITE_OK);
    CHECK(verify_rows(database) == 0);
    DB_CHECK(database, sqlite3_close(database) == SQLITE_OK);
    DB_CHECK(database, sqlite3_open_v2(path, &database, SQLITE_OPEN_READONLY,
                                      "scarlet-native") == SQLITE_OK);
    CHECK(verify_rows(database) == 0);
    CHECK(sqlite3_exec(database, "DELETE FROM items", NULL, NULL, NULL) == SQLITE_READONLY);
    DB_CHECK(database, sqlite3_close(database) == SQLITE_OK);
    CHECK(puts("SCARLET_SQLITE REOPEN PASS") >= 0);
    return 0;
}

#define WORKER_COUNT 4
#define WORKER_ITERATIONS 64

typedef struct SharedWork {
    ThreadGate *gate;
    sqlite3 *database;
    int worker;
    int failed_line;
} SharedWork;

static void *shared_connection_worker(void *argument) {
    SharedWork *work = argument;
    sqlite3_stmt *statement = NULL;
    sqlite3 *private_database = NULL;
    sqlite3_mutex *mutex;
    int index;
    int result;
    int saved_errno = 0x400 + work->worker;
    errno = saved_errno;
    worker_gate(work->gate);
    if (errno != saved_errno) {
        work->failed_line = __LINE__;
        return argument;
    }
    /* Different connection mutexes cannot serialize this phase. SQLite's
     * shared allocator and PRNG mutexes must protect their own global state. */
    result = sqlite3_open_v2(":memory:", &private_database,
        SQLITE_OPEN_READWRITE | SQLITE_OPEN_CREATE | SQLITE_OPEN_FULLMUTEX,
        "scarlet-native");
    if (result != SQLITE_OK || scalar_integer(private_database,
        "WITH RECURSIVE n(i) AS (VALUES(1) UNION ALL SELECT i+1 FROM n WHERE i<64) "
        "SELECT sum(length(randomblob(17))) FROM n", 64 * 17) != 0)
        work->failed_line = __LINE__;
    if (private_database != NULL && sqlite3_close(private_database) != SQLITE_OK)
        work->failed_line = __LINE__;
    if (work->failed_line != 0) return argument;
    /* Each worker owns its statement, while the same serialized connection
     * arbitrates concurrent prepare/bind/step/reset/finalize calls. There is
     * deliberately no application mutex around this first phase. */
    result = sqlite3_prepare_v2(work->database,
        "INSERT INTO work VALUES(?1,?2,?1*1000+?2,randomblob(17))",
        -1, &statement, NULL);
    if (result != SQLITE_OK) {
        work->failed_line = __LINE__;
        return argument;
    }
    for (index = 0; index < WORKER_ITERATIONS; ++index) {
        if (sqlite3_bind_int(statement, 1, work->worker) != SQLITE_OK
            || sqlite3_bind_int(statement, 2, index) != SQLITE_OK
            || sqlite3_step(statement) != SQLITE_DONE
            || sqlite3_reset(statement) != SQLITE_OK) {
            work->failed_line = __LINE__;
            break;
        }
    }
    if (sqlite3_finalize(statement) != SQLITE_OK) work->failed_line = __LINE__;
    if (work->failed_line != 0) return argument;
    /* A multi-call transaction requires an application-held connection mutex
     * so another worker cannot accidentally join that transaction. This also
     * verifies recursive lock and trylock through SQLite's actual backend. */
    mutex = sqlite3_db_mutex(work->database);
    sqlite3_mutex_enter(mutex);
    if (sqlite3_mutex_try(mutex) != SQLITE_OK) {
        work->failed_line = __LINE__;
    } else {
        sqlite3_mutex_leave(mutex);
        if (sqlite3_exec(work->database,
            "BEGIN; UPDATE tally SET total=total+1; COMMIT;",
            NULL, NULL, NULL) != SQLITE_OK) {
            work->failed_line = __LINE__;
            (void)sqlite3_exec(work->database, "ROLLBACK", NULL, NULL, NULL);
        }
    }
    sqlite3_mutex_leave(mutex);
    return argument;
}

static int shared_connection_test(void) {
    sqlite3 *database = NULL;
    ThreadGate gate = THREAD_GATE_INITIALIZER;
    SharedWork work[WORKER_COUNT];
    pthread_t threads[WORKER_COUNT];
    int index;
    int returned_correctly = 1;
    DB_CHECK(database, sqlite3_open_v2(":memory:", &database,
        SQLITE_OPEN_READWRITE | SQLITE_OPEN_CREATE | SQLITE_OPEN_FULLMUTEX,
        "scarlet-native") == SQLITE_OK);
    CHECK(sqlite3_db_mutex(database) != NULL);
    SQL(database, "CREATE TABLE work(worker INTEGER,iteration INTEGER,checksum INTEGER,"
        "payload BLOB,PRIMARY KEY(worker,iteration));"
        "CREATE TABLE tally(total INTEGER); INSERT INTO tally VALUES(0);");
    for (index = 0; index < WORKER_COUNT; ++index) {
        work[index].gate = &gate;
        work[index].database = database;
        work[index].worker = index;
        work[index].failed_line = 0;
        THREAD_CALL(pthread_create(&threads[index], NULL,
                                    shared_connection_worker, &work[index]));
    }
    gate_wait(&gate, WORKER_COUNT);
    gate_release(&gate);
    for (index = 0; index < WORKER_COUNT; ++index) {
        void *returned = NULL;
        THREAD_CALL(pthread_join(threads[index], &returned));
        if (returned != &work[index]) returned_correctly = 0;
    }
    gate_destroy(&gate);
    CHECK(returned_correctly);
    for (index = 0; index < WORKER_COUNT; ++index) {
        if (work[index].failed_line != 0)
            printf("SCARLET_SQLITE worker=%d failed_line=%d\n", index, work[index].failed_line);
        CHECK(work[index].failed_line == 0);
    }
    CHECK(scalar_integer(database, "SELECT count(*) FROM work",
                          WORKER_COUNT * WORKER_ITERATIONS) == 0);
    CHECK(scalar_integer(database, "SELECT sum(checksum) FROM work",
        WORKER_ITERATIONS * 1000 * (WORKER_COUNT * (WORKER_COUNT - 1) / 2)
        + WORKER_COUNT * WORKER_ITERATIONS * (WORKER_ITERATIONS - 1) / 2) == 0);
    CHECK(scalar_integer(database, "SELECT count(*) FROM work WHERE length(payload)=17",
                          WORKER_COUNT * WORKER_ITERATIONS) == 0);
    CHECK(scalar_integer(database, "SELECT total FROM tally", WORKER_COUNT) == 0);
    CHECK(scalar_text(database, "PRAGMA integrity_check", "ok") == 0);
    DB_CHECK(database, sqlite3_close(database) == SQLITE_OK);
    CHECK(puts("SCARLET_SQLITE SHARED_CONNECTION_THREADS PASS") >= 0);
    return 0;
}

typedef struct ContendingWork {
    ThreadGate *gate;
    const char *path;
    int failed_line;
} ContendingWork;

static void *contending_connection_worker(void *argument) {
    ContendingWork *work = argument;
    sqlite3 *database = NULL;
    int result = sqlite3_open_v2(work->path, &database,
        SQLITE_OPEN_READWRITE | SQLITE_OPEN_FULLMUTEX, "scarlet-native");
    if (result != SQLITE_OK) {
        work->failed_line = __LINE__;
    } else if (sqlite3_exec(database, "SELECT count(*) FROM items", NULL, NULL, NULL)
                   != SQLITE_BUSY
        || sqlite3_exec(database, "BEGIN IMMEDIATE", NULL, NULL, NULL) != SQLITE_BUSY) {
        work->failed_line = __LINE__;
    }
    /* Tell the main thread both BUSY attempts finished while it still owns
     * its transaction. Wait until it has actually rolled back before retry. */
    worker_gate(work->gate);
    if (work->failed_line == 0
        && (scalar_integer(database, "SELECT count(*) FROM items", 24) != 0
            || sqlite3_exec(database, "BEGIN IMMEDIATE; ROLLBACK;",
                            NULL, NULL, NULL) != SQLITE_OK))
        work->failed_line = __LINE__;
    if (database != NULL && sqlite3_close(database) != SQLITE_OK)
        work->failed_line = __LINE__;
    return argument;
}

static int contending_connection_test(const char *path) {
    sqlite3 *database = NULL;
    ThreadGate gate = THREAD_GATE_INITIALIZER;
    ContendingWork work = { &gate, path, 0 };
    pthread_t thread;
    void *returned = NULL;
    DB_CHECK(database, sqlite3_open_v2(path, &database,
        SQLITE_OPEN_READWRITE | SQLITE_OPEN_FULLMUTEX, "scarlet-native") == SQLITE_OK);
    SQL(database, "BEGIN IMMEDIATE;");
    THREAD_CALL(pthread_create(&thread, NULL, contending_connection_worker, &work));
    gate_wait(&gate, 1);
    /* A recoverable return here would strand a worker on a dead stack gate. */
    if (execute(database, "ROLLBACK;", __LINE__) != SQLITE_OK) abort();
    gate_release(&gate);
    THREAD_CALL(pthread_join(thread, &returned));
    gate_destroy(&gate);
    CHECK(returned == &work);
    if (work.failed_line != 0)
        printf("SCARLET_SQLITE contending_worker failed_line=%d\n", work.failed_line);
    CHECK(work.failed_line == 0);
    CHECK(scalar_integer(database, "SELECT count(*) FROM items", 24) == 0);
    DB_CHECK(database, sqlite3_close(database) == SQLITE_OK);
    CHECK(puts("SCARLET_SQLITE CONTENDING_CONNECTION_THREADS PASS") >= 0);
    return 0;
}

static int read_exact(int fd, unsigned char *data, size_t amount) {
    size_t done = 0;
    while (done < amount) {
        ssize_t count = pread(fd, data + done, amount - done, (off_t)done);
        if (count < 0 && errno == EINTR) continue;
        CHECK(count > 0 && (size_t)count <= amount - done);
        done += (size_t)count;
    }
    return 0;
}

static int crash_database(const char *path) {
    sqlite3 *database = NULL;
    char journal[1024];
    unsigned char magic[8];
    const unsigned char expected_magic[8] = {0xd9, 0xd5, 0x05, 0xf9, 0x20, 0xa1, 0x63, 0xd7};
    unsigned char *before;
    unsigned char *after;
    off_t size;
    int fd;
    int length = snprintf(journal, sizeof(journal), "%s-journal", path);
    CHECK(length > 0 && (size_t)length < sizeof(journal));
    fd = open(path, O_RDONLY);
    CHECK(fd >= 0);
    size = lseek(fd, 0, SEEK_END);
    CHECK(size > 0 && size < 8 * 1024 * 1024);
    before = malloc((size_t)size);
    after = malloc((size_t)size);
    CHECK(before != NULL && after != NULL);
    CHECK(read_exact(fd, before, (size_t)size) == 0);
    CHECK(close(fd) == 0);
    DB_CHECK(database, sqlite3_open_v2(path, &database, SQLITE_OPEN_READWRITE,
                                      "scarlet-native") == SQLITE_OK);
    CHECK(scalar_text(database, "PRAGMA journal_mode=DELETE", "delete") == 0);
    SQL(database, "PRAGMA synchronous=FULL; PRAGMA cache_size=3; PRAGMA cache_spill=ON;"
        "BEGIN IMMEDIATE; UPDATE items SET payload=zeroblob(131113) WHERE id=7;"
        "UPDATE items SET score=score+1000.0;");
    /* Force modified pages through xWrite before terminating. A transaction
     * that only changed RAM would not establish rollback recovery. */
    DB_CHECK(database, sqlite3_db_cacheflush(database) == SQLITE_OK);
    fd = open(path, O_RDONLY);
    CHECK(fd >= 0);
    CHECK(read_exact(fd, after, (size_t)size) == 0);
    CHECK(memcmp(before, after, (size_t)size) != 0);
    CHECK(close(fd) == 0);
    free(before);
    free(after);
    fd = open(journal, O_RDONLY);
    CHECK(fd >= 0 && lseek(fd, 0, SEEK_END) > 512);
    CHECK(read_exact(fd, magic, sizeof(magic)) == 0);
    CHECK(memcmp(magic, expected_magic, sizeof(magic)) == 0);
    CHECK(close(fd) == 0);
    CHECK(puts("SCARLET_SQLITE DIRTY_DATABASE_AND_HOT_JOURNAL PASS") >= 0);
    CHECK(puts("SCARLET_LIBC_SQLITE_CRASH_READY") >= 0);
    CHECK(fflush(stdout) == 0);
    /* Deliberately leave SQLite's transaction, file handles, and real kernel
     * lock open. The next process must acquire/recover through normal SQLite. */
    abort();
}

int main(int argc, char **argv) {
    char path[1024];
    int create;
    int crash;
    CHECK((argc == 2 || argc == 3) && argv[1] != NULL && argv[1][0] != '\0');
    create = argc == 2 || strcmp(argv[2], "create") == 0;
    crash = argc == 3 && strcmp(argv[2], "crash") == 0;
    CHECK(create || crash || strcmp(argv[2], "verify") == 0);
    CHECK(strcmp(sqlite3_libversion(), "3.53.4") == 0);
    CHECK(sqlite3_threadsafe() == 1);
    CHECK(scarlet_sqlite_configure() == SQLITE_OK);
    CHECK(sqlite3_initialize() == SQLITE_OK);
    CHECK(make_path(path, sizeof(path), argv[1], "sqlite-roundtrip.db") == 0);
    if (crash) return crash_database(path);
    if (create) {
        CHECK(vfs_test(argv[1]) == 0);
        CHECK(create_database(path) == 0);
        CHECK(shared_connection_test() == 0);
        CHECK(contending_connection_test(path) == 0);
        CHECK(puts("SCARLET_LIBC_SQLITE_PTHREAD_OK") >= 0);
    }
    CHECK(reopen_database(path) == 0);
    CHECK(sqlite3_shutdown() == SQLITE_OK);
    CHECK(puts(create ? "SCARLET_SQLITE CREATE PASS" : "SCARLET_SQLITE VERIFY PASS") >= 0);
    CHECK(puts("SCARLET_SQLITE 3.53.4 PASS") >= 0);
    CHECK(puts("SCARLET_LIBC_SQLITE_OK") >= 0);
    return 53;
}
