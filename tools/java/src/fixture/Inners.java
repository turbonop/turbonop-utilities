package fixture;

import java.util.Comparator;
import java.util.function.IntSupplier;

/** Nested classes of every kind: member, static, local, anonymous, and their nest. */
public class Inners {
    private int secret = 7;

    private static int hidden() {
        return 3;
    }

    public class Member {
        int peek() {
            return secret + hidden();
        }

        class Deeper {
            int deeper() {
                return secret + peek();
            }
        }
    }

    static class Nested {
        protected interface Callback {
            void call(int x);
        }
    }

    private static final class PrivateFinal {}

    abstract static class Abstract {}

    IntSupplier local(int offset) {
        class Local implements IntSupplier {
            public int getAsInt() {
                return secret + offset;
            }
        }
        return new Local();
    }

    Comparator<String> anonymous() {
        return new Comparator<String>() {
            @Override
            public int compare(String a, String b) {
                return a.length() - b.length() + secret;
            }
        };
    }

    static Runnable inStatic() {
        return new Runnable() {
            public void run() {
                hidden();
            }
        };
    }

    Object inInitializer = new Object() {
        @Override
        public String toString() {
            return "init";
        }
    };

    IntSupplier lambda() {
        return () -> secret * 2;
    }
}
