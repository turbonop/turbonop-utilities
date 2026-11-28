package fixture;

/** An interface with abstract, default, static, and private methods, and a constant. */
public interface Iface {
    int LIMIT = 10;
    String NAME = "iface";

    String describe();

    default String twice() {
        return helper() + helper();
    }

    static Iface of(String s) {
        return () -> s;
    }

    private String helper() {
        return describe();
    }

    private static int count() {
        return LIMIT;
    }
}
