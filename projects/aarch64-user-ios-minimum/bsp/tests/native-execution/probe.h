#pragma once

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include <sys/types.h>

typedef void (*ProbeLog)(const char *line);
bool probe_run(const char *fixture, ProbeLog log);

typedef struct {
    uint64_t counter;
    uint64_t *self;
    uint64_t (*function)(uint64_t);
} GuestData;

typedef struct {
    void *base;
    GuestData *data;
    uint64_t *bss;
    uint64_t *tls;
    uint64_t *heap;
    uintptr_t sp;
    uintptr_t entry, checkpoint, spin, tls_write, tls_read;
    uint64_t generation;
} Guest;

typedef struct {
    void *library;
    int fd;
    size_t page, text_size, data_offset, data_filesize, data_size, slot_size;
    off_t text_fileoff, data_fileoff;
    size_t counter, bss, helper, entry, checkpoint, spin, tls_write, tls_read;
    unsigned reserved_slots, rx_mappings, memory_operations;
    size_t reserved_bytes, live_rw_bytes;
    Guest guests[2];
} ImagePool;

bool image_open(ImagePool *pool, const char *path, ProbeLog log);
bool image_acquire(ImagePool *pool, Guest *guest, ProbeLog log);
bool image_release(ImagePool *pool, Guest *guest, ProbeLog log);
bool image_is_fresh(const Guest *guest);
void image_seed(Guest *guest, unsigned task);
// Keep the registered file and executable mappings alive for the process.
void image_close_file(ImagePool *pool);

bool carrier_test(ImagePool *pool, ProbeLog log);
void probe_log(ProbeLog log, const char *format, ...) __attribute__((format(printf, 2, 3)));
