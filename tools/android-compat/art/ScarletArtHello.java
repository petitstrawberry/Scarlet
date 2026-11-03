/** A plain Java/DEX probe: no Android framework or application dependencies. */
public final class ScarletArtHello {
    static int mix(int value) {
        return (value * 31 + 7) ^ (value >>> 3);
    }

    public static void main(String[] args) {
        System.out.println("SCARLET_ART_BEGIN");
        int answer = 0;
        for (int i = 0; i < 7; i++) {
            answer += 6;
        }
        if (answer != 42) {
            throw new AssertionError("interpreter arithmetic");
        }
        int hot = 1;
        for (int i = 0; i < 2000000; i++) {
            hot = mix(hot);
        }
        System.out.println("Scarlet ART: " + answer + " hot=" + (hot & 0xffff));
        System.out.println("SCARLET_ART_OK");
    }
}
