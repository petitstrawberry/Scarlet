/* SPDX-License-Identifier: MIT */
public final class Jit {
    static final class Box { int value = 17; }
    static long work(int n) {
        long result = 0;
        for (int i = 0; i < n; i++) result += (long)i * i + 3 * i;
        return result;
    }
    static int load(Box box) { return box.value; }
    static int recurse(int n) { return 1 + recurse(n + 1); }
    public static void main(String[] args) {
        if (System.getProperty("java.vm.name").contains("Zero")) throw new AssertionError("JIT VM required");
        for (int n = 0; n < 1000; n++) {
            long expected = (long)n * (n - 1) * (2 * n - 1) / 6 + 3L * n * (n - 1) / 2;
            if (work(n) != expected) throw new AssertionError("compiled arithmetic");
        }
        if (load(new Box()) != 17) throw new AssertionError("field load");
        try { load(null); throw new AssertionError("missing null check"); }
        catch (NullPointerException expected) { }
        try { recurse(0); throw new AssertionError("missing stack guard"); }
        catch (StackOverflowError expected) { }
        if (work(1000) != 334332000L) throw new AssertionError("recovery after exceptions");
        System.out.println("OPENJDK_JIT PASS arithmetic=ok null=ok stack=ok");
    }
}
