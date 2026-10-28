/* SPDX-License-Identifier: MIT */
import java.util.concurrent.atomic.AtomicInteger;
import java.util.concurrent.atomic.AtomicReference;

public final class JitStress {
    static final int THREADS = 4, ROUNDS = 8;
    static final AtomicInteger arrivals = new AtomicInteger();
    static final AtomicReference<Throwable> failure = new AtomicReference<>();
    static volatile int phase;
    static volatile boolean abort;

    static final class Box {
        int value;
        Box(int value) { this.value = value; }
    }

    static int field(Box box) { return box.value; }
    static int recurse(int depth) { return 1 + recurse(depth + 1); }

    static void round(Box live, Box[] roots, int worker, int round) {
        byte[] buffer = new byte[32768];
        buffer[0] = (byte)(worker + round);
        try { field(null); throw new AssertionError("missing null exception"); }
        catch (NullPointerException expected) { }
        if (round == 1) {
            try { recurse(0); throw new AssertionError("missing stack exception"); }
            catch (StackOverflowError expected) { }
        }
        arrivals.incrementAndGet();
        // The controller collects only after all four workers enter here.
        // Keep object/array references live in this compiled frame across GC.
        while (phase < round && !abort) {
            if (field(live) != worker + 17) throw new AssertionError("live object");
            Thread.onSpinWait();
        }
        if (field(live) != worker + 17 || buffer[0] != (byte)(worker + round)) {
            throw new AssertionError("compiled roots after GC");
        }
        for (int i = 0; i < roots.length; i++) {
            if (field(roots[i]) != worker * 100 + i) throw new AssertionError("array roots after GC");
        }
    }

    static void worker(int id) {
        try {
            Box live = new Box(id + 17);
            Box[] roots = new Box[32];
            for (int i = 0; i < roots.length; i++) roots[i] = new Box(id * 100 + i);
            for (int round = 1; round <= ROUNDS && !abort; round++) round(live, roots, id, round);
        } catch (Throwable error) {
            failure.compareAndSet(null, error);
            abort = true;
        }
    }

    public static void main(String[] args) throws Exception {
        if (System.getProperty("java.vm.name").contains("Zero")) throw new AssertionError("JIT required");
        Thread[] workers = new Thread[THREADS];
        try {
            for (int i = 0; i < workers.length; i++) {
                final int id = i;
                workers[i] = new Thread(() -> worker(id), "jit-stress-" + id);
                workers[i].start();
            }
            long deadline = System.nanoTime() + 60_000_000_000L;
            for (int round = 1; round <= ROUNDS; round++) {
                while (arrivals.get() < THREADS * round) {
                    if (failure.get() != null) throw new AssertionError("worker failed", failure.get());
                    if (System.nanoTime() > deadline) throw new AssertionError("workers did not rendezvous");
                    Thread.yield();
                }
                System.gc();
                phase = round;
            }
        } finally {
            // Release compiled loops on failure as well as normal completion.
            phase = Integer.MAX_VALUE;
            for (Thread worker : workers) {
                if (worker != null) {
                    worker.join(5000);
                    if (worker.isAlive()) throw new AssertionError("worker did not exit");
                }
            }
        }
        if (failure.get() != null) throw new AssertionError("worker failed", failure.get());
        if (arrivals.get() != THREADS * ROUNDS) throw new AssertionError("incomplete rounds");
        System.out.println("OPENJDK_JIT_STRESS PASS threads=4 gc=8 null=32 stack=4 roots=ok");
    }
}
