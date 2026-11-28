package legacy;

import java.util.ArrayList;
import java.util.List;

/**
 * Compiled for Java 8 (class version 52): string concatenation with
 * StringBuilder, synthetic accessors for private members (no nestmates),
 * an enum switch's synthetic map, and an assertion.
 */
public class Legacy {
    private int secret = 1;
    private static String name = "legacy";

    enum Mode { ON, OFF }

    class Inner {
        int peek() {
            secret++;
            return secret + name.length();
        }
    }

    static String concat(String a, int b, double c, Object d) {
        return a + b + c + d + 'x';
    }

    static int mode(Mode m) {
        switch (m) {
            case ON: return 1;
            case OFF: return 2;
            default: return 0;
        }
    }

    int check(int x) {
        assert x > 0 : "positive";
        List<Runnable> rs = new ArrayList<>();
        rs.add(() -> secret++);
        return x;
    }
}
