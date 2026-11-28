package preview;

/** Uses a preview feature of Java 21 (unnamed variables): class version 65.65535. */
public class Preview {
    static int count(Iterable<String> items) {
        int n = 0;
        for (String _ : items) {
            n++;
        }
        return n;
    }
}
