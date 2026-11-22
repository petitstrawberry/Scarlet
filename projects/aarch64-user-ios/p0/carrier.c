#include "probe.h"

#include <dispatch/dispatch.h>
#include <errno.h>
#include <mach/arm/thread_status.h>
#include <pthread.h>
#include <signal.h>
#include <stdatomic.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/ucontext.h>

#define ALT_STACK_SIZE (128 * 1024)
#define NZCV_MASK 0xf0000000u

extern void probe_enter(void);
extern void probe_return(void);

typedef struct {
    uint64_t x[31];
    uintptr_t sp, pc;
    uint32_t nzcv, fpcr, fpsr;
    __uint128_t q[32];
} Registers;

typedef enum { HOST, ENTERING, GUEST, EXITING } Mode;
typedef enum { CHECKPOINT, TLS_WRITE, TLS_READ, TIMER, GUEST_FAULT } Reason;

typedef struct {
    uintptr_t text_start, text_end;
    uintptr_t checkpoint, tls_write, tls_read;
    uint64_t epoch, generation;
    unsigned task;
    Registers registers;
} Run;

typedef struct {
    Registers registers;
    uint64_t epoch, generation;
    unsigned task;
    Reason reason;
    uint32_t esr;
    uintptr_t far;
    bool entered;
} Stop;

typedef struct {
    atomic_uint mode;
    atomic_uint_fast64_t interrupt_epoch;
    dispatch_semaphore_t request, completed;
    void *altstack;
    bool shutdown;
    int setup_error;
    Run run;
    Stop stop;
    Registers host;
} Carrier;

static const int signals[] = {SIGTRAP, SIGUSR1, SIGSEGV, SIGBUS, SIGILL, SIGFPE};
static struct sigaction previous[sizeof signals / sizeof signals[0]];
static _Atomic(Carrier *) active;

static void capture(Registers *result, const struct __darwin_mcontext64 *context) {
    const arm_thread_state64_t *state = &context->__ss;
    memcpy(result->x, state->__x, sizeof state->__x);
    result->x[29] = arm_thread_state64_get_fp(*state);
    result->x[30] = arm_thread_state64_get_lr(*state);
    result->sp = arm_thread_state64_get_sp(*state);
    result->pc = arm_thread_state64_get_pc(*state);
    result->nzcv = state->__cpsr & NZCV_MASK;
    memcpy(result->q, context->__ns.__v, sizeof result->q);
    result->fpcr = context->__ns.__fpcr;
    result->fpsr = context->__ns.__fpsr;
}

static void restore(struct __darwin_mcontext64 *context, const Registers *source, uint64_t x18) {
    arm_thread_state64_t *state = &context->__ss;
    memcpy(state->__x, source->x, sizeof state->__x);
    state->__x[18] = x18;
    arm_thread_state64_set_fp(*state, source->x[29]);
    arm_thread_state64_set_lr_fptr(*state, (void (*)(void))source->x[30]);
    arm_thread_state64_set_sp(*state, source->sp);
    arm_thread_state64_set_pc_fptr(*state, (void (*)(void))source->pc);
    state->__cpsr = (state->__cpsr & ~NZCV_MASK) | source->nzcv;
    memcpy(context->__ns.__v, source->q, sizeof source->q);
    context->__ns.__fpcr = source->fpcr;
    context->__ns.__fpsr = source->fpsr;
}

static void forward_signal(int signal, siginfo_t *info, void *raw) {
    for (size_t i = 0; i < sizeof signals / sizeof signals[0]; ++i) {
        if (signals[i] != signal) continue;
        const struct sigaction *action = &previous[i];
        if (action->sa_handler == SIG_IGN) return;
        if (action->sa_handler == SIG_DFL) {
            sigaction(signal, action, NULL);
            // The current handler blocks this signal until sigreturn.
            raise(signal);
        } else if (action->sa_flags & SA_SIGINFO) {
            action->sa_sigaction(signal, info, raw);
        } else {
            action->sa_handler(signal);
        }
        return;
    }
}

static void finish(Carrier *carrier, struct __darwin_mcontext64 *context, Reason reason,
                   bool was_running) {
    const Run *run = &carrier->run;
    Stop *stop = &carrier->stop;
    atomic_store(&carrier->mode, EXITING);
    if (was_running) {
        capture(&stop->registers, context);
        stop->registers.x[18] = run->registers.x[18];
    } else {
        stop->registers = run->registers;
    }
    stop->task = run->task;
    stop->generation = run->generation;
    stop->epoch = run->epoch;
    stop->reason = reason;
    stop->esr = context->__es.__esr;
    stop->far = context->__es.__far;
    stop->entered = was_running;
    restore(context, &carrier->host, carrier->host.x[18]);
}

static void handle_signal(int signal, siginfo_t *info, void *raw) {
    Carrier *carrier = atomic_load(&active);
    uintptr_t marker = (uintptr_t)&raw;
    if (!carrier || marker < (uintptr_t)carrier->altstack ||
        marker - (uintptr_t)carrier->altstack >= ALT_STACK_SIZE) {
        forward_signal(signal, info, raw);
        return;
    }
    struct __darwin_mcontext64 *context = ((ucontext_t *)raw)->uc_mcontext;
    uintptr_t pc = arm_thread_state64_get_pc(context->__ss);
    uint32_t esr = context->__es.__esr;
    uint32_t brk = signal == SIGTRAP && esr >> 26 == 0x3c ? esr & 0xffff : 0;
    Mode mode = atomic_load(&carrier->mode);
    const Run *run = &carrier->run;
    uint64_t requested = atomic_load(&carrier->interrupt_epoch);

    if (mode == ENTERING && pc == (uintptr_t)probe_enter && brk == 0x5600) {
        capture(&carrier->host, context);
        carrier->host.pc = (uintptr_t)probe_return;
        if (requested == run->epoch) {
            finish(carrier, context, TIMER, false);
        } else {
            restore(context, &run->registers, carrier->host.x[18]);
            atomic_store(&carrier->mode, GUEST);
        }
        return;
    }
    if (signal == SIGUSR1 && requested != 0) {
        if (mode == GUEST && requested == run->epoch) finish(carrier, context, TIMER, true);
        // A notification arriving before entry is checked by the entry BRK.
        return;
    }
    if (mode == GUEST && pc >= run->text_start && pc < run->text_end) {
        Reason reason = GUEST_FAULT;
        if (pc == run->checkpoint && brk == 0x5101) reason = CHECKPOINT;
        if (pc == run->tls_write && brk == 0x5500) reason = TLS_WRITE;
        if (pc == run->tls_read && brk == 0x5400) reason = TLS_READ;
        finish(carrier, context, reason, true);
        return;
    }
    forward_signal(signal, info, raw);
}

static void *carrier_main(void *opaque) {
    Carrier *carrier = opaque;
    const stack_t stack = {.ss_sp = carrier->altstack, .ss_size = ALT_STACK_SIZE};
    stack_t previous_stack;
    if (sigaltstack(&stack, &previous_stack) != 0) {
        carrier->setup_error = errno;
        dispatch_semaphore_signal(carrier->completed);
        return NULL;
    }
    sigset_t mask, original_mask;
    sigemptyset(&mask);
    sigaddset(&mask, SIGUSR1);
    carrier->setup_error = pthread_sigmask(SIG_UNBLOCK, &mask, &original_mask);
    dispatch_semaphore_signal(carrier->completed);
    if (!carrier->setup_error) {
        for (;;) {
            dispatch_semaphore_wait(carrier->request, DISPATCH_TIME_FOREVER);
            if (carrier->shutdown) break;
            atomic_store(&carrier->mode, ENTERING);
            probe_enter();
            atomic_store(&carrier->mode, HOST);
            dispatch_semaphore_signal(carrier->completed);
        }
        pthread_sigmask(SIG_SETMASK, &original_mask, NULL);
    }
    sigaltstack(&previous_stack, NULL);
    return NULL;
}

static bool run_guest(Carrier *carrier, pthread_t thread, const Guest *guest,
                      unsigned task, size_t text_size, Registers *registers,
                      Reason expected, ProbeLog log) {
    carrier->run = (Run){
        .text_start = (uintptr_t)guest->base,
        .text_end = (uintptr_t)guest->base + text_size,
        .checkpoint = guest->checkpoint,
        .tls_write = guest->tls_write,
        .tls_read = guest->tls_read,
        .epoch = carrier->run.epoch + 1,
        .generation = guest->generation,
        .task = task,
        .registers = *registers,
    };
    dispatch_semaphore_signal(carrier->request);
    int64_t delay = expected == TIMER ? 10 * NSEC_PER_MSEC : NSEC_PER_SEC;
    if (dispatch_semaphore_wait(carrier->completed, dispatch_time(DISPATCH_TIME_NOW, delay))) {
        atomic_store(&carrier->interrupt_epoch, carrier->run.epoch);
        int error = pthread_kill(thread, SIGUSR1);
        if (error || dispatch_semaphore_wait(carrier->completed,
                                            dispatch_time(DISPATCH_TIME_NOW, NSEC_PER_SEC))) {
            probe_log(log, "Carrier failed to stop (pthread_kill=%d)", error);
            // The carrier may still execute guest code. Keep its state alive.
            abort();
        }
    }
    const Stop *stop = &carrier->stop;
    if (stop->epoch != carrier->run.epoch || stop->generation != guest->generation ||
        stop->task != task || stop->reason != expected || !stop->entered) {
        probe_log(log, "Unexpected stop (task=%u gen=%llu epoch=%llu reason=%u wanted=%u pc=%p esr=%#x far=%p)",
                  stop->task, (unsigned long long)stop->generation, (unsigned long long)stop->epoch,
                  stop->reason, expected, (void *)stop->registers.pc, stop->esr, (void *)stop->far);
        return false;
    }
    *registers = stop->registers;
    return true;
}

static bool check_entry(Carrier *carrier, pthread_t thread, ImagePool *pool,
                        unsigned task, ProbeLog log) {
    Guest *guest = &pool->guests[task];
    Registers entry = {.sp = guest->sp, .pc = guest->entry};
    entry.x[2] = (uintptr_t)guest->heap;
    entry.x[30] = guest->checkpoint;
    if (!run_guest(carrier, thread, guest, task, pool->text_size, &entry, CHECKPOINT, log)) {
        return false;
    }
    if (entry.x[0] != 48 + task || guest->data->counter != 41 + task || *guest->bss != 1 ||
        entry.x[1] != 0x3330 + task || entry.x[2] != 0x2220 + task) {
        probe_log(log, "Guest %u did not execute against its own DATA/BSS/GOT/stack/heap", task);
        return false;
    }
    return true;
}

static bool check_registers(const Registers *actual, const Registers *expected,
                            unsigned task, ProbeLog log) {
    for (unsigned i = 0; i < 31; ++i) {
        if (actual->x[i] != expected->x[i]) {
            probe_log(log, "Guest %u lost x%u (got %#llx, wanted %#llx)", task, i,
                      (unsigned long long)actual->x[i], (unsigned long long)expected->x[i]);
            return false;
        }
    }
    for (unsigned i = 0; i < 32; ++i) {
        if (actual->q[i] != expected->q[i]) {
            probe_log(log, "Guest %u lost q%u", task, i);
            return false;
        }
    }
    if (actual->sp != expected->sp || actual->pc != expected->pc ||
        actual->nzcv != expected->nzcv || actual->fpcr != expected->fpcr ||
        actual->fpsr != expected->fpsr) {
        probe_log(log, "Guest %u state mismatch (sp=%p pc=%p nzcv=%#x fpcr=%#x fpsr=%#x)",
                  task, (void *)actual->sp, (void *)actual->pc, actual->nzcv,
                  actual->fpcr, actual->fpsr);
        return false;
    }
    return true;
}

static bool round_trip(Carrier *carrier, pthread_t thread, ImagePool *pool, ProbeLog log) {
    Registers states[2] = {0};
    uint64_t virtual_tls[2] = {0};
    unsigned memory_operations = pool->memory_operations;
    for (unsigned i = 0; i < 2; ++i) {
        Guest *guest = &pool->guests[i];
        if (!check_entry(carrier, thread, pool, i, log)) return false;
        Registers *state = &states[i];
        for (unsigned j = 0; j < 31; ++j) state->x[j] = 0x9641ae0300f00000ull + 31 * i + j;
        for (unsigned j = 0; j < 32; ++j) {
            state->q[j] = ((__uint128_t)(0x96c7458e67891200ull + 32 * i + j) << 64) |
                          (0x12345678990abc00ull + 32 * i + j);
        }
        state->x[0] = (uintptr_t)guest->tls;
        state->x[29] = guest->sp - 16;
        state->x[30] = guest->entry;
        state->sp = guest->sp - 32;
        state->pc = guest->tls_write;
        state->nzcv = i ? 0xa0000000 : 0x50000000;
        state->fpcr = i ? 0x800000 : 0x400000;
        state->fpsr = i ? 0x08000004 : 0x10;

        Registers trap = *state;
        if (!run_guest(carrier, thread, guest, i, pool->text_size, &trap, TLS_WRITE, log) ||
            !check_registers(&trap, state, i, log)) return false;
        virtual_tls[i] = trap.x[0];
        trap.pc += 4;
        trap.x[0] = 0;
        if (trap.pc != guest->tls_read || virtual_tls[i] != (uintptr_t)guest->tls) {
            probe_log(log, "Guest %u virtual TLS write did not reach the expected boundary", i);
            return false;
        }
        *state = trap;
    }
    log("PASS both RX aliases execute with private DATA, BSS and function pointers");

    for (unsigned i = 0; i < 2; ++i) {
        Guest *guest = &pool->guests[i];
        Registers *state = &states[i];
        Registers trap = *state;
        if (!run_guest(carrier, thread, guest, i, pool->text_size, &trap, TLS_READ, log)) return false;
        trap.x[0] = virtual_tls[i];
        trap.pc += 4;
        state->x[0] = (uintptr_t)guest->tls;
        state->pc = guest->checkpoint;
        if (!run_guest(carrier, thread, guest, i, pool->text_size, &trap, CHECKPOINT, log) ||
            !check_registers(&trap, state, i, log)) {
            return false;
        }
        state->pc = guest->spin;
    }

    for (unsigned turn = 0; turn < 32; ++turn) {
        unsigned i = turn % 2;
        Guest *guest = &pool->guests[i];
        const Registers *expected = &states[i];
        Registers stopped = *expected;
        if (!run_guest(carrier, thread, guest, i, pool->text_size, &stopped, TIMER, log)) return false;
        if (!check_registers(&stopped, expected, i, log) ||
            virtual_tls[i] != (uintptr_t)guest->tls || *guest->tls != 0x1110 + i) {
            probe_log(log, "Guest %u lost registers or TLS at timer stop %u", i, turn);
            return false;
        }
        states[i] = stopped;
    }
    if (pool->memory_operations != memory_operations) {
        log("Mappings changed during a guest context switch");
        return false;
    }
    log("PASS 32 alternating timer stops, full register state, zero guest VM operations");
    return true;
}

static bool exercise_reuse(Carrier *carrier, pthread_t thread, ImagePool *pool, ProbeLog log) {
    void *bases[] = {pool->guests[0].base, pool->guests[1].base};
    for (unsigned round = 0; round < 64; ++round) {
        for (unsigned i = 0; i < 2; ++i) {
            if (!image_release(pool, &pool->guests[i], log)) return false;
        }
        if (pool->live_rw_bytes || pool->reserved_slots != 2 || pool->rx_mappings != 2 ||
            pool->reserved_bytes != 2 * pool->slot_size) {
            probe_log(log, "Pool did not return to two idle RX slots (round=%u)", round);
            return false;
        }
        for (unsigned i = 0; i < 2; ++i) {
            Guest *guest = &pool->guests[i];
            if (!image_acquire(pool, guest, log)) return false;
            if (guest->base != bases[i] || guest->generation != round + 2 || !image_is_fresh(guest)) {
                probe_log(log, "Guest %u slot did not reset (round=%u)", i, round);
                return false;
            }
            image_seed(guest, i);
            if (!check_entry(carrier, thread, pool, i, log)) return false;
        }
    }
    probe_log(log, "PASS 64 release and re-execute cycles, %zu bytes of retained VA across two RX slots",
              pool->reserved_bytes);
    return true;
}

bool carrier_test(ImagePool *pool, ProbeLog log) {
    // Process-wide handlers can still be returning on other threads at teardown.
    static Carrier carrier;
    carrier.request = dispatch_semaphore_create(0);
    carrier.completed = dispatch_semaphore_create(0);
    carrier.altstack = mmap(NULL, ALT_STACK_SIZE, PROT_READ | PROT_WRITE,
                            MAP_PRIVATE | MAP_ANON, -1, 0);
    unsigned installed = 0;
    bool passed = false;
    if (carrier.altstack == MAP_FAILED || !carrier.request || !carrier.completed) {
        log("Could not allocate carrier control state");
        goto done;
    }
    struct sigaction action = {
        .sa_sigaction = handle_signal,
        .sa_flags = SA_SIGINFO | SA_ONSTACK | SA_RESTART,
    };
    sigfillset(&action.sa_mask);
    for (; installed < sizeof signals / sizeof signals[0]; ++installed) {
        if (sigaction(signals[installed], &action, &previous[installed]) != 0) {
            probe_log(log, "sigaction(%d): %s", signals[installed], strerror(errno));
            goto done;
        }
    }
    atomic_store(&active, &carrier);
    pthread_t thread;
    int error = pthread_create(&thread, NULL, carrier_main, &carrier);
    if (error) {
        probe_log(log, "pthread_create: %s", strerror(error));
        goto done;
    }
    dispatch_semaphore_wait(carrier.completed, DISPATCH_TIME_FOREVER);
    if (carrier.setup_error) {
        probe_log(log, "Could not prepare carrier signal stack or mask: %s", strerror(carrier.setup_error));
    } else {
        passed = round_trip(&carrier, thread, pool, log) &&
                 exercise_reuse(&carrier, thread, pool, log);
        carrier.shutdown = true;
        dispatch_semaphore_signal(carrier.request);
    }
    pthread_join(thread, NULL);
done:
    atomic_store(&active, NULL);
    while (installed > 0) {
        --installed;
        sigaction(signals[installed], &previous[installed], NULL);
    }
    if (carrier.altstack != MAP_FAILED) munmap(carrier.altstack, ALT_STACK_SIZE);
    if (carrier.completed) dispatch_release(carrier.completed);
    if (carrier.request) dispatch_release(carrier.request);
    return passed;
}
