#include "probe.h"

#include <stdarg.h>
#include <stdio.h>
#include <sys/sysctl.h>
#include <sys/types.h>
#include <unistd.h>

void probe_log(ProbeLog log, const char *format, ...) {
    char buffer[512];
    va_list args;
    va_start(args, format);
    vsnprintf(buffer, sizeof buffer, format, args);
    va_end(args);
    log(buffer);
}

static bool debugger_absent(ProbeLog log) {
    struct kinfo_proc process;
    size_t size = sizeof process;
    int mib[] = {CTL_KERN, KERN_PROC, KERN_PROC_PID, getpid()};
    if (sysctl(mib, 4, &process, &size, NULL, 0) != 0 || size != sizeof process) {
        log("Could not check whether the process is being debugged");
        return false;
    }
    if (process.kp_proc.p_flag & P_TRACED) {
        log("Debugger attached. Relaunch the probe without it.");
        return false;
    }
    return true;
}

static bool release_pair(ImagePool *pool, ProbeLog log) {
    bool ok = true;
    for (unsigned i = 0; i < 2; ++i) {
        if (pool->guests[i].data && !image_release(pool, &pool->guests[i], log)) ok = false;
    }
    return ok;
}

static bool run_pool(ImagePool *pool, ProbeLog log) {
    for (unsigned i = 0; i < 2; ++i) {
        Guest *guest = &pool->guests[i];
        if (!image_acquire(pool, guest, log)) return false;
        if (!image_is_fresh(guest)) {
            probe_log(log, "Guest %u did not start with fresh writable pages", i);
            return false;
        }
        image_seed(guest, i);
    }
    if (pool->guests[0].base == pool->guests[1].base) {
        log("Guest slots have the same base address");
        return false;
    }
    log("Entering the RX aliases on a dedicated carrier pthread");
    return carrier_test(pool, log);
}

bool probe_run(const char *fixture, ProbeLog log) {
    char build[64] = "unknown";
    size_t size = sizeof build;
    sysctlbyname("kern.osversion", build, &size, NULL, 0);
    probe_log(log, "Scarlet P0 (OS build %s)", build);
    if (!debugger_absent(log)) return false;

    ImagePool pool;
    bool passed = image_open(&pool, fixture, log) && run_pool(&pool, log);
    if (!release_pair(&pool, log)) passed = false;
    image_close_file(&pool);
    return passed;
}
