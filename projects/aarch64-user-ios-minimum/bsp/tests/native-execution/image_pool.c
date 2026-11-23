#include "probe.h"

#include <dlfcn.h>
#include <errno.h>
#include <fcntl.h>
#include <mach-o/loader.h>
#include <string.h>
#include <sys/mman.h>
#include <unistd.h>

static bool file_error(ProbeLog log, const char *operation) {
    int error = errno;
    probe_log(log, "%s: errno=%d (%s)", operation, error, strerror(error));
    return false;
}

static bool symbols(ImagePool *pool, uintptr_t base, ProbeLog log) {
    const struct {
        const char *name;
        size_t *offset;
        size_t lower, upper, length;
    } entries[] = {
        {"entry", &pool->entry, 0, pool->text_size, 4},
        {"helper", &pool->helper, 0, pool->text_size, 4},
        {"checkpoint", &pool->checkpoint, 0, pool->text_size, 4},
        {"spin", &pool->spin, 0, pool->text_size, 4},
        {"tls_write", &pool->tls_write, 0, pool->text_size, 4},
        {"tls_read", &pool->tls_read, 0, pool->text_size, 4},
        {"data", &pool->counter, pool->data_offset,
         pool->data_offset + pool->data_filesize, sizeof(GuestData)},
        {"bss", &pool->bss, pool->data_offset,
         pool->data_offset + pool->data_size, sizeof(uint64_t)},
    };
    for (size_t i = 0; i < sizeof entries / sizeof entries[0]; ++i) {
        char name[64] = "scarlet_fixture_";
        strlcat(name, entries[i].name, sizeof name);
        uintptr_t address = (uintptr_t)dlsym(pool->library, name);
        if (address < base || address - base < entries[i].lower ||
            address - base > entries[i].upper ||
            entries[i].length > entries[i].upper - (address - base)) {
            probe_log(log, "Invalid fixture symbol: %s", name);
            return false;
        }
        *entries[i].offset = address - base;
    }
    return true;
}

bool image_open(ImagePool *pool, const char *path, ProbeLog log) {
    *pool = (ImagePool){.fd = -1};
    long page = sysconf(_SC_PAGESIZE);
    if (page != 16384) {
        probe_log(log, "Fixture requires 16 KiB pages (host=%ld)", page);
        return false;
    }
    pool->page = (size_t)page;
    pool->library = dlopen(path, RTLD_NOW | RTLD_LOCAL | RTLD_FIRST);
    if (!pool->library) {
        probe_log(log, "dlopen: %s", dlerror());
        return false;
    }

    Dl_info info;
    void *entry = dlsym(pool->library, "scarlet_fixture_entry");
    if (!entry || !dladdr(entry, &info)) {
        log("Could not locate the dyld image");
        return false;
    }
    const struct mach_header_64 *header = info.dli_fbase;
    const unsigned char *commands = (const unsigned char *)(header + 1);
    const struct segment_command_64 *text = NULL, *data = NULL;
    // Load commands belong to the already validated, bundled fixture.
    for (uint32_t i = 0, offset = 0; i < header->ncmds; ++i) {
        const struct load_command *command = (const void *)(commands + offset);
        if (command->cmd == LC_SEGMENT_64) {
            const struct segment_command_64 *segment = (const void *)command;
            if (!strncmp(segment->segname, "__TEXT", sizeof segment->segname)) text = segment;
            if (!strncmp(segment->segname, "__DATA", sizeof segment->segname)) data = segment;
        }
        offset += command->cmdsize;
    }
    if (header->magic != MH_MAGIC_64 || !text || !data ||
        text->initprot != (VM_PROT_READ | VM_PROT_EXECUTE) ||
        data->initprot != (VM_PROT_READ | VM_PROT_WRITE) ||
        text->filesize != text->vmsize || !text->vmsize ||
        text->fileoff % page || text->vmsize % page ||
        data->vmaddr < text->vmaddr + text->vmsize ||
        (data->vmaddr - text->vmaddr) % page ||
        !data->vmsize || data->vmsize % page || data->filesize > data->vmsize) {
        log("Unexpected fixture segment layout");
        return false;
    }
    pool->text_fileoff = (off_t)text->fileoff;
    pool->text_size = text->vmsize;
    pool->data_fileoff = (off_t)data->fileoff;
    pool->data_offset = data->vmaddr - text->vmaddr;
    pool->data_filesize = data->filesize;
    pool->data_size = data->vmsize;
    // DATA, TLS, heap, one inaccessible guard page, and one stack page.
    pool->slot_size = pool->data_offset + pool->data_size + 4 * page;
    if (!symbols(pool, (uintptr_t)header, log)) return false;
    pool->fd = open(info.dli_fname, O_RDONLY | O_CLOEXEC);
    if (pool->fd < 0) return file_error(log, "open(dyld image)");
    return true;
}

static bool mapped(ImagePool *pool, void *address, size_t length, int protection,
                   int fd, off_t offset, ProbeLog log, const char *name) {
    ++pool->memory_operations;
    if (mmap(address, length, protection, MAP_PRIVATE | MAP_FIXED |
             (fd < 0 ? MAP_ANON : 0), fd, offset) == MAP_FAILED) {
        return file_error(log, name);
    }
    return true;
}

static bool hide_writable(ImagePool *pool, Guest *guest, ProbeLog log) {
    unsigned char *base = guest->base;
    return mapped(pool, base + pool->data_offset, pool->data_size + 4 * pool->page,
                  PROT_NONE, -1, 0, log, "mmap(retire writable pages)");
}

static bool copy_template(ImagePool *pool, void *data, ProbeLog log) {
    ++pool->memory_operations;
    ssize_t size = pread(pool->fd, data, pool->data_filesize, pool->data_fileoff);
    if (size < 0) return file_error(log, "pread(DATA template)");
    if ((size_t)size != pool->data_filesize) {
        log("Short read of DATA template");
        return false;
    }
    return true;
}

bool image_acquire(ImagePool *pool, Guest *guest, ProbeLog log) {
    if (guest->data != NULL) return false;
    if (!guest->base) {
        ++pool->memory_operations;
        void *base = mmap(NULL, pool->slot_size, PROT_NONE, MAP_PRIVATE | MAP_ANON, -1, 0);
        if (base == MAP_FAILED) return file_error(log, "mmap(reserve slot)");
        if (!mapped(pool, base, pool->text_size, PROT_READ | PROT_EXEC,
                    pool->fd, pool->text_fileoff, log, "mmap(signed __TEXT, RX)")) {
            ++pool->memory_operations;
            munmap(base, pool->slot_size);
            return false;
        }
        guest->base = base;
        ++pool->reserved_slots;
        ++pool->rx_mappings;
        pool->reserved_bytes += pool->slot_size;
        probe_log(log, "Mapped signed RX at %p (%zu bytes)", base, pool->text_size);
    }
    unsigned char *base = guest->base;
    size_t page = pool->page;
    unsigned char *data = base + pool->data_offset;
    unsigned char *extra = data + pool->data_size;
    if (!mapped(pool, data, pool->data_size + 2 * page, PROT_READ | PROT_WRITE,
                -1, 0, log, "mmap(DATA, TLS, heap)") ||
        !mapped(pool, extra + 3 * page, page, PROT_READ | PROT_WRITE,
                -1, 0, log, "mmap(stack)") ||
        !copy_template(pool, data, log)) {
        hide_writable(pool, guest, log);
        return false;
    }
    guest->data = (GuestData *)(base + pool->counter);
    guest->bss = (uint64_t *)(base + pool->bss);
    guest->data->self = &guest->data->counter;
    guest->data->function = (void *)(base + pool->helper);
    guest->tls = (uint64_t *)extra;
    guest->heap = (uint64_t *)(extra + page);
    guest->sp = (uintptr_t)(extra + 4 * page);
    guest->entry = (uintptr_t)(base + pool->entry);
    guest->checkpoint = (uintptr_t)(base + pool->checkpoint);
    guest->spin = (uintptr_t)(base + pool->spin);
    guest->tls_write = (uintptr_t)(base + pool->tls_write);
    guest->tls_read = (uintptr_t)(base + pool->tls_read);
    ++guest->generation;
    pool->live_rw_bytes += pool->data_size + 3 * page;
    return true;
}

bool image_release(ImagePool *pool, Guest *guest, ProbeLog log) {
    if (!guest->data || !hide_writable(pool, guest, log)) return false;
    pool->live_rw_bytes -= pool->data_size + 3 * pool->page;
    guest->data = NULL;
    guest->bss = NULL;
    guest->tls = NULL;
    guest->heap = NULL;
    return true;
}

bool image_is_fresh(const Guest *guest) {
    return guest->data->counter == 40 && guest->data->self == &guest->data->counter &&
           guest->data->function != NULL && *guest->bss == 0 && *guest->tls == 0 &&
           *guest->heap == 0 && *(uint64_t *)(guest->sp - 8) == 0;
}

void image_seed(Guest *guest, unsigned task) {
    guest->data->counter += task;
    *guest->tls = 0x1110 + task;
    *guest->heap = 0x2220 + task;
    *(uint64_t *)(guest->sp - 8) = 0x3330 + task;
}

void image_close_file(ImagePool *pool) {
    if (pool->fd >= 0) close(pool->fd);
    if (pool->library && !pool->rx_mappings) dlclose(pool->library);
}
