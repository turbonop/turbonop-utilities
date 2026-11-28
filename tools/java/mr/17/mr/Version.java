package mr;

/** The multi-release JAR's version for Java 17 and later. */
public class Version {
    public static String get() {
        record R(String s) {}
        return new R("17").s();
    }
}
