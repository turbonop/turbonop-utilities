package fixture;

import java.io.IOException;
import java.io.Serializable;
import java.util.ArrayList;
import java.util.List;
import java.util.Map;
import java.util.function.Function;
import java.util.function.IntBinaryOperator;
import java.util.function.Supplier;

/** A little of every bytecode javac writes, and every kind of constant. */
public class Everything implements Serializable, Comparable<Everything> {
    private static final long serialVersionUID = 42L;

    // ConstantValue of each type.
    public static final int INT = 123456;
    public static final long LONG = 1234567890123L;
    public static final float FLOAT = 1.5f;
    public static final double DOUBLE = 2.25;
    public static final String STRING = "a \"quoted\" string\twith\nescapes é 日";
    public static final char CHAR = 'x';
    public static final boolean BOOLEAN = true;
    public static final byte BYTE = -7;
    public static final short SHORT = 1000;
    public static final float NAN = Float.NaN;
    public static final double INF = Double.NEGATIVE_INFINITY;
    public static final float NEG_ZERO = -0.0f;
    public static final double SMALL = 4.9e-324;
    public static final long MIN_LONG = Long.MIN_VALUE;
    public static final int MIN_INT = Integer.MIN_VALUE;

    static int counter;
    static Object lock = new Object();

    protected volatile int vol;
    transient long trans;
    private final List<String> names = new ArrayList<>();
    Map<String, ? extends List<Integer>> map;
    int[][] grid;
    long wide;
    double dbl;

    public Everything() {
        this(0);
    }

    public Everything(int vol) {
        this.vol = vol;
    }

    static {
        counter = INT / 3;
    }

    // Constants pushed every way.
    static Object[] constants() {
        return new Object[] {
            -1, 0, 1, 2, 3, 4, 5, 6, 127, -128, 128, 32767, -32768, 32768, 0L, 1L, 2L,
            0.0f, 1.0f, 2.0f, 3.0f, 0.0, 1.0, 2.0, null, "s", Everything.class, int[].class,
            LONG, DOUBLE, 'c',
        };
    }

    // Arithmetic and conversions of every type.
    static double arithmetic(int i, long l, float f, double d, byte b, short s, char c) {
        int i2 = i + 1 - 2 * 3 / (i | 1) % 7;
        i2 = -i2 ^ (i2 & 0xF0) | (i2 << 3) >> 1 >>> 2;
        long l2 = l + 1 - 2 * 3 / (l | 1) % 7;
        l2 = -l2 ^ (l2 & 0xF0) | (l2 << 3) >> 1 >>> 2;
        float f2 = f + 1 - 2 * 3 / f % 7;
        f2 = -f2;
        double d2 = d + 1 - 2 * 3 / d % 7;
        d2 = -d2;
        i2 += (int) l2 + (int) f2 + (int) d2;
        l2 += (long) i2 + (long) f2 + (long) d2;
        f2 += (float) i2 + (float) l2 + (float) d2;
        d2 += (double) i2 + (double) l2 + (double) f2;
        b = (byte) i2;
        s = (short) i2;
        c = (char) i2;
        i2++;
        i2 += 100;
        i2 -= 1000;
        return i2 + l2 + f2 + d2 + b + s + c;
    }

    // Comparisons and branches of every kind.
    static int compare(int a, int b, long c, long d, float e, float f, double g, double h, Object o,
            Object p) {
        int n = 0;
        if (a == b) n++;
        if (a != b) n++;
        if (a < b) n++;
        if (a >= b) n++;
        if (a > b) n++;
        if (a <= b) n++;
        if (a == 0) n++;
        if (a != 0) n++;
        if (a < 0) n++;
        if (a >= 0) n++;
        if (a > 0) n++;
        if (a <= 0) n++;
        if (c < d) n++;
        if (e < f) n++;
        if (e > f) n++;
        if (g < h) n++;
        if (g > h) n++;
        if (o == p) n++;
        if (o != p) n++;
        if (o == null) n++;
        if (o != null) n++;
        return n;
    }

    // Arrays of every element type.
    static int arrays(int n) {
        boolean[] z = new boolean[n];
        byte[] b = new byte[n];
        char[] c = new char[n];
        short[] s = new short[n];
        int[] i = new int[n];
        long[] l = new long[n];
        float[] f = new float[n];
        double[] d = new double[n];
        String[] a = new String[n];
        int[][][] m = new int[n][n][];
        long[][] ml = new long[2][3];
        z[0] = true;
        b[0] = 1;
        c[0] = 'c';
        s[0] = 2;
        i[0] = 3;
        l[0] = 4;
        f[0] = 5;
        d[0] = 6;
        a[0] = "7";
        m[0][0] = i;
        ml[1][2] = l[0]++;
        l[1] += l[0];
        d[1] = d[0] = 8;
        return (z[0] ? 1 : 0) + b[0] + c[0] + s[0] + i[0] + (int) l[0] + (int) f[0] + (int) d[0]
                + a.length + m[0][0].length + (int) ml[1][2];
    }

    // Switches: tableswitch, lookupswitch, strings, enums, patterns.
    static int switches(int k, String s, Color color, Object o) {
        int r;
        switch (k) {
            case 1: r = 10; break;
            case 2: r = 20; break;
            case 3: r = 30; break;
            case 5: r = 50; break;
            default: r = 0;
        }
        switch (k) {
            case -1000: r += 1; break;
            case 7: r += 2; break;
            case 100000: r += 3; break;
            default: r += 4;
        }
        r += switch (s) {
            case "alpha" -> 1;
            case "beta", "gamma" -> 2;
            default -> 3;
        };
        r += switch (color) {
            case RED -> 1;
            case GREEN -> 2;
            case BLUE -> 3;
        };
        r += switch (o) {
            case Integer i when i > 10 -> i;
            case Integer i -> -i;
            case String t -> t.length();
            case Point(int x, int y, var t) -> x + y + t.size();
            case null -> 0;
            default -> 99;
        };
        return r;
    }

    // Objects: new, fields, casts, instanceof, monitors, exceptions.
    int objects(Object o) throws IOException, InterruptedException {
        Everything e = new Everything(3);
        e.vol = this.vol + 1;
        e.wide = this.wide++;
        this.dbl += e.dbl;
        counter++;
        if (o instanceof String s && !s.isEmpty()) {
            return s.length();
        }
        CharSequence cs = (CharSequence) o;
        synchronized (lock) {
            counter += cs.length();
        }
        try {
            if (counter > 10) {
                throw new IOException("big");
            }
        } catch (IllegalStateException | IllegalArgumentException x) {
            counter = 0;
        } catch (RuntimeException x) {
            throw x;
        } finally {
            counter--;
        }
        return e.vol;
    }

    // invokedynamic: lambdas, method references, string concatenation.
    static String lambdas(int base) {
        IntBinaryOperator add = (a, b) -> a + b + base;
        Function<String, Integer> len = String::length;
        Supplier<List<String>> make = ArrayList::new;
        Runnable r = () -> counter++;
        r.run();
        List<String> list = make.get();
        list.add("x");
        return "sum=" + add.applyAsInt(1, 2) + ", len=" + len.apply("four") + ", list=" + list
                + ", base=" + base + '!' + 1.5 + true;
    }

    // Loops (back branches, frames) and a long method's extended frames.
    static long loops(int[] values) {
        long sum = 0;
        for (int v : values) {
            sum += v;
        }
        int i = 0;
        while (i < values.length) {
            if (values[i] < 0) {
                break;
            }
            i++;
        }
        do {
            sum--;
        } while (sum > 1000);
        outer:
        for (int a = 0; a < 3; a++) {
            for (int b = 0; b < 3; b++) {
                if (a * b > 2) continue outer;
                sum += a * b;
            }
        }
        return sum;
    }

    @Override
    public int compareTo(Everything o) {
        return Integer.compare(vol, o.vol);
    }

    @Deprecated(since = "1.0", forRemoval = true)
    public void old() {}

    /** @deprecated javadoc's tag alone */
    @SuppressWarnings("dep-ann")
    public void older() {}

    public static synchronized native void nativeMethod(int[] a, String... rest);

    public strictfp double strict(double a) {
        return a * 2;
    }

    @SafeVarargs
    public static <T> List<T> varargs(T... items) {
        return List.of(items);
    }

    public static void main(String[] args) throws Exception {
        System.out.println(new Everything().objects("hello"));
    }
}
