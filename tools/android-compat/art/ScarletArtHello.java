/** A plain Java/DEX probe: no Android framework or application dependencies. */
public final class ScarletArtHello {
    public static void main(String[] args) {
        System.out.println("SCARLET_ART_BEGIN");
        int answer = 0;
        for (int i = 0; i < 7; i++) {
            answer += 6;
        }
        if (answer != 42) {
            throw new AssertionError("interpreter arithmetic");
        }
        System.out.println("Scarlet ART: " + answer);
        System.out.println("SCARLET_ART_OK");
    }
}
