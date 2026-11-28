// What testdata/dex/Indy's call sites and method handles name (see
// MakeIndy.java, which writes the class that uses them).

package dex.sample;

import java.lang.invoke.CallSite;
import java.lang.invoke.ConstantCallSite;
import java.lang.invoke.MethodHandles;
import java.lang.invoke.MethodType;

public class Bootstraps implements Runnable {
    public static int staticField;
    public long instanceField;

    public Bootstraps() {}

    public static CallSite bootstrap(MethodHandles.Lookup lookup, String name, MethodType type,
            Object... args) throws ReflectiveOperationException {
        return new ConstantCallSite(lookup.findStatic(Bootstraps.class, "target", type));
    }

    public static Object target() {
        return "target";
    }

    public static long target(int a, String b) {
        return a + b.length();
    }

    public void run() {}

    private void secret() {}
}
