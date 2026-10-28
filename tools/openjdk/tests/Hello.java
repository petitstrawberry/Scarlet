/* SPDX-License-Identifier: MIT */
import java.io.FileInputStream;
import java.io.FileOutputStream;
import java.util.concurrent.atomic.AtomicInteger;

public final class Hello {
    static void check(boolean ok, String what) {
        if (!ok) throw new AssertionError(what);
    }
    public static void main(String[] args) throws Exception {
        check(args.length == 2 && "Scarlet".equals(args[0]) && "いちご🍓".equals(args[1]), "UTF-8 arguments");
        check("Scarlet".equals(System.getProperty("os.name")), "native OS identity");
        String vm = System.getProperty("java.vm.name");
        check(vm.contains("Zero") || vm.contains("Server") || vm.contains("Client"), "HotSpot VM identity");
        check("/root".equals(System.getenv("HOME")), "inherited environment");
        System.out.println("Hello from OpenJDK on Scarlet!");
        check(Math.sqrt(144.0) == 12.0 && Double.isNaN(Math.sqrt(-1)), "floating point");
        int[] values = {1, 2, 3, 4, 5};
        System.arraycopy(values, 0, values, 1, 4);
        check(values[0] == 1 && values[1] == 1 && values[4] == 4, "overlapping arraycopy");
        AtomicInteger count = new AtomicInteger();
        AtomicInteger threadErrors = new AtomicInteger();
        ThreadLocal<Integer> local = new ThreadLocal<>();
        Thread[] workers = new Thread[4];
        for (int t = 0; t < workers.length; t++) {
            final int id = t;
            workers[t] = new Thread(() -> {
                local.set(id);
                for (int i = 0; i < 1000; i++) {
                    count.incrementAndGet();
                    if ((i & 31) == 0) Thread.yield();
                }
                check(local.get() == id, "thread local isolation");
            });
            workers[t].setUncaughtExceptionHandler((thread, error) -> threadErrors.incrementAndGet());
            workers[t].start();
        }
        for (Thread worker : workers) worker.join();
        check(threadErrors.get() == 0, "uncaught thread errors");
        check(count.get() == 4000, "threads and atomics");
        long checksum = 0;
        for (int i = 0; i < 512; i++) {
            byte[] data = new byte[256 * 1024];
            data[0] = (byte)i;
            data[data.length-1] = (byte)(i+1);
            checksum += data[0] + data[data.length-1];
        }
        check(checksum == -512, "allocation and GC");
        System.gc();
        byte[] payload = new byte[98317];
        for (int i = 0; i < payload.length; i++) payload[i] = (byte)(i*31);
        try (FileOutputStream out = new FileOutputStream("/tmp/openjdk-hello.bin")) {
            out.write(payload);
        }
        try (FileInputStream in = new FileInputStream("/tmp/openjdk-hello.bin")) {
            byte[] actual = in.readAllBytes();
            check(java.util.Arrays.equals(actual, payload), "file I/O across 64 KiB");
            check(in.read() == -1, "file EOF");
        }
        System.out.println("OPENJDK_JAVA PASS threads=4 atomic=4000 allocated=128MiB files=ok");
    }
}
