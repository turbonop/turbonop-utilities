// The sample classes (testdata/dex/Sample-*.dex): a little of everything a
// DEX file can hold, for turbonop-dex's tests. See build.cmd.
// (ASCII only: other characters are escapes.)

package dex.sample;

import java.io.IOException;
import java.io.Serializable;
import java.lang.annotation.ElementType;
import java.lang.annotation.Retention;
import java.lang.annotation.RetentionPolicy;
import java.lang.annotation.Target;
import java.lang.invoke.MethodHandle;
import java.lang.invoke.MethodHandles;
import java.lang.invoke.MethodType;
import java.lang.invoke.VarHandle;
import java.util.ArrayList;
import java.util.List;
import java.util.Map;
import java.util.function.IntBinaryOperator;
import java.util.function.Supplier;

/** Every kind of annotation element (and so of encoded value). */
@Retention(RetentionPolicy.RUNTIME)
@Target({ElementType.TYPE, ElementType.METHOD, ElementType.FIELD, ElementType.PARAMETER,
    ElementType.CONSTRUCTOR, ElementType.LOCAL_VARIABLE, ElementType.TYPE_USE})
@interface Everything {
    byte b() default -1;
    short s() default -300;
    char c() default '\u20ac';
    int i() default 0x12345678;
    long j() default -0x123456789abcdefL;
    float f() default 1.5f;
    double d() default -2.75e100;
    boolean z() default true;
    String str() default "default \"text\"";
    Class<?> type() default int[][].class;
    Color color() default Color.GREEN;
    Note note() default @Note("nested");
    int[] ints() default {1, 2, 3};
    String[] strings() default {};
    Class<?>[] types() default {String.class, void.class};
    Color[] colors() default {Color.RED, Color.BLUE};
    Note[] notes() default {@Note("a"), @Note(value = "b", level = 2)};
}

/** Kept in the class file, not at run time: VISIBILITY_BUILD. */
@Retention(RetentionPolicy.CLASS)
@interface Note {
    String value();
    int level() default 1;
}

enum Color {
    RED, GREEN, BLUE;

    Color next() {
        return values()[(ordinal() + 1) % values().length];
    }
}

interface Shape extends Serializable {
    double PI_ISH = 3.14;

    double area();

    default String describe() {
        return "shape of area " + area();
    }

    static Shape unit() {
        return () -> 1.0;
    }
}

abstract class Base<T extends Comparable<T>> implements Shape {
    protected T value;

    abstract T make(int n) throws IOException, IllegalStateException;

    native long nativeCall(int a, long b);

    public double area() {
        return 0;
    }
}

@Everything(b = 7, s = 8, c = 'x', i = -9, j = 10L, f = Float.NaN, d = Double.NEGATIVE_INFINITY,
    z = false, str = "\u00e9t\u00e9 \ud83d\ude00", type = Map.Entry.class, color = Color.BLUE,
    note = @Note(value = "deep", level = 3), ints = {}, strings = {"x", "y"},
    types = {long.class}, colors = {}, notes = {@Note("one")})
@Note("on the class")
public class Sample extends Base<String> implements Cloneable {
    // Static values of every kind.
    public static final byte B = -128;
    public static final short S = 32767;
    public static final char C = '\uffff';
    public static final int I = Integer.MIN_VALUE;
    public static final long J = Long.MAX_VALUE;
    public static final float F = -0.0f;
    public static final double D = 1e-300;
    public static final boolean Z = true;
    public static final String STR = "static \"quoted\"\n\t\u0000\u0001\u007f\u0080\u07ff\u0800\uffff";
    public static final String SURROGATES = "\ud83d\ude00 \udbff\udfff \ud800 lone";
    public static final String CONTROLS = "\u0001\u0002\u0003\u0004\u0005\u0006\u0007\u0008\u0009\n\u000b\u000c\r\u000e\u000f\u0010\u0011\u0012\u0013\u0014\u0015\u0016\u0017\u0018\u0019\u001a\u001b\u001c\u001d\u001e\u001f\u007f";
    public static final int SMALL = 5;
    public static final long LONG_SMALL = 1;
    public static final float FLOAT_ONE = 1.0f;
    public static final double DOUBLE_INT = 3.0;
    public static final char CHAR_A = 'A';
    public static final boolean NO = false;
    public static int counter;
    public static Object nothing;
    static volatile long ticks;
    static String computed = String.valueOf(System.nanoTime());
    static final int[] TABLE = {1, 1, 2, 3, 5, 8, 13, 21, 34, 55};
    static final long[] LONGS = {1L << 40, -1L, 0x7fffffffffffffffL};
    static final byte[] BYTES = {1, -2, 3, -4, 5};
    static final short[] SHORTS = {1000, -1000, 32767};
    static final char[] CHARS = {'a', 'b', '\u1234'};
    static final boolean[] BOOLS = {true, false, true};
    static final float[] FLOATS = {1.5f, -2.5f, 1e30f};
    static final double[] DOUBLES = {Math.PI, Math.E, -0.0};

    @Everything
    private transient int instanceInt;
    protected long instanceLong;
    double instanceDouble;
    private final List<Map<String, ? super Integer>> generic = new ArrayList<>();
    boolean flag;
    byte byteField;
    char charField;
    short shortField;
    float floatField;
    Sample next;
    Sample[] siblings;

    public Sample() {}

    @Everything(i = 1)
    protected Sample(@Note("param") int a, @Everything(z = false) String b, long c) {
        this.instanceInt = a;
        this.value = b;
        this.instanceLong = c;
    }

    @Override
    String make(int n) {
        return "made " + n;
    }

    // Arithmetic of every kind, with literals of every width.
    static int ints(int a, int b) {
        int r = a + b;
        r -= a * b;
        r /= (b | 1);
        r %= 7;
        r &= 0xff00ff;
        r |= 0x1000;
        r ^= a;
        r <<= 3;
        r >>= 1;
        r >>>= 2;
        r = -r;
        r = ~r;
        r += 100;
        r += 1000;
        r += 100000;
        r *= -5;
        r = 200 - r;
        r = 20000 - r;
        r = r / 3 + r % 300 + (r & 127) + (r | 9) + (r ^ 1000);
        r = (r << 5) + (r >> 6) + (r >>> 7);
        return r + (a << b) + (a >> b) + (a >>> b);
    }

    static long longs(long a, long b, int s) {
        long r = a + b - a * b;
        r /= (b | 1);
        r %= 11;
        r = (r & a) | (r ^ b);
        r = (r << s) + (r >> s) + (r >>> s);
        r = -r + ~r;
        if (a < b) {
            r++;
        }
        return r + 0x123456789L + (1L << 50) + 0x7fffffff + 1000000;
    }

    static double floats(float a, double b) {
        float f = a + 1.5f;
        f = f - a * 2.0f / 3.0f % 4.0f;
        double d = b + f;
        d = d * b - d / b % 7.0;
        d = -d;
        if (a < 1.0f || a > 2.0f || b <= 3.0 || b >= 4.0 || a == f || b != d) {
            d += 1e10;
        }
        return d + f + 123456.789f + 0.1;
    }

    static long conversions(int i, long j, float f, double d) {
        byte b = (byte) i;
        char c = (char) i;
        short s = (short) i;
        long r = (long) i + (long) f + (long) d;
        r += (int) j + (int) f + (int) d;
        float f2 = (float) i + (float) j + (float) d;
        double d2 = (double) i + (double) j + (double) f;
        return r + b + c + s + (long) f2 + (long) d2;
    }

    // Constants: const/4, /16, plain, /high16; wide /16, /32, plain, /high16.
    static long constants() {
        int a = 7;
        int b = -8;
        int c = 1234;
        int d = 0x12345678;
        int e = 0x7f000000;
        long f = 7L;
        long g = -32768L;
        long h = 0x12345678L;
        long i = 0x123456789abcdefL;
        long j = 0x7ff0000000000000L;
        float k = 2.0f;
        double l = 2.0;
        counter = a + b + c + d + e;
        return f + g + h + i + j + (long) k + (long) l;
    }

    // Comparisons and branches (if-*z, if-*).
    static int branches(int a, int b, Object o) {
        int r = 0;
        if (a == 0) r++;
        if (a != 0) r += 2;
        if (a < 0) r += 3;
        if (a >= 0) r += 4;
        if (a > 0) r += 5;
        if (a <= 0) r += 6;
        if (a == b) r += 7;
        if (a != b) r += 8;
        if (a < b) r += 9;
        if (a >= b) r += 10;
        if (a > b) r += 11;
        if (a <= b) r += 12;
        if (o == null) r += 13;
        if (o != null) r += 14;
        if (o instanceof String) r += ((String) o).length();
        return r;
    }

    // Switches: packed, sparse, on strings and enums.
    static int packed(int x) {
        switch (x) {
            case 10: return 1;
            case 11: return 4;
            case 12: return 9;
            case 13: return 16;
            case 15: return 25;
            default: return -1;
        }
    }

    static int sparse(int x) {
        switch (x) {
            case -1000000: return 1;
            case -5: return 2;
            case 0: return 3;
            case 77: return 4;
            case 100000: return 5;
            case Integer.MAX_VALUE: return 6;
            default: return 0;
        }
    }

    static int strings(String s) {
        switch (s) {
            case "alpha": return 1;
            case "beta": return 2;
            case "gamma": return 3;
            default: return 0;
        }
    }

    static int colors(Color c) {
        switch (c) {
            case RED: return 1;
            case GREEN: return 2;
            default: return 3;
        }
    }

    // Arrays: every element type's new-array, aget, aput, fill-array-data,
    // filled-new-array.
    static long arrays(int n) {
        int[] ia = new int[n];
        long[] la = new long[n];
        byte[] ba = new byte[n];
        char[] ca = new char[n];
        short[] sa = new short[n];
        boolean[] za = new boolean[n];
        float[] fa = new float[n];
        double[] da = new double[n];
        Object[] oa = new Object[n];
        int[][] grid = new int[n][n];
        String[][][] cube = new String[2][3][];
        ia[0] = 1;
        la[0] = 2;
        ba[0] = 3;
        ca[0] = 'c';
        sa[0] = 5;
        za[0] = true;
        fa[0] = 7;
        da[0] = 8;
        oa[0] = "o";
        grid[0][0] = ia.length;
        cube[0][0] = new String[] {"s"};
        int[] lit = {9, 8, 7, 6, 5, 4};
        int[] few = {n, n + 1, n + 2};
        int[] many = {n, n, n, n, n, n, n};
        return ia[0] + la[0] + ba[0] + ca[0] + sa[0] + (za[0] ? 1 : 0) + (long) fa[0] + (long) da[0]
            + oa.length + grid[0][0] + cube.length + lit[1] + few[2] + many[6] + TABLE[3]
            + LONGS[0] + BYTES[1] + SHORTS[2] + CHARS[0] + (BOOLS[1] ? 1 : 0) + (long) FLOATS[0]
            + (long) DOUBLES[1];
    }

    // Exceptions: several handlers, catch-all, finally, nested, rethrow,
    // multi-catch, try-with-resources.
    static int exceptions(String s) throws IOException {
        int r = 0;
        try {
            r = Integer.parseInt(s);
        } catch (NumberFormatException e) {
            r = -1;
        } catch (IllegalArgumentException | IllegalStateException e) {
            r = -2;
        } finally {
            counter++;
        }
        try {
            try {
                if (r > 5) {
                    throw new IOException("big " + r);
                }
            } catch (RuntimeException e) {
                throw new IllegalStateException(e);
            }
        } catch (Error e) {
            r = e.hashCode();
        }
        try (java.io.StringReader rd = new java.io.StringReader(s)) {
            r += rd.read();
        }
        return r;
    }

    // Monitors, instance-of, check-cast, const-class, throw.
    synchronized int locked(Object o) {
        synchronized (o) {
            if (o instanceof Sample[]) {
                return ((Sample[]) o).length;
            }
            if (o == Sample.class || o == int.class || o == String[].class) {
                throw new IllegalArgumentException();
            }
            return ((CharSequence) o).length();
        }
    }

    // Field access of every width, instance and static.
    long fields() {
        instanceInt++;
        instanceLong += 2;
        instanceDouble *= 1.5;
        flag = !flag;
        byteField++;
        charField++;
        shortField++;
        floatField += 1;
        next = this;
        siblings = new Sample[] {this, next};
        counter += instanceInt;
        ticks++;
        nothing = generic;
        return instanceInt + instanceLong + (long) instanceDouble + (flag ? 1 : 0) + byteField
            + charField + shortField + (long) floatField + counter + ticks + siblings.length;
    }

    // Calls: virtual, super, direct, static, interface; and /range (more
    // than five argument words).
    static long many(int a, long b, double c, Object d, String e, char f, byte g, short h) {
        return a + b + (long) c + d.hashCode() + e.length() + f + g + h;
    }

    long calls(Shape s, List<String> list) {
        long r = many(1, 2, 3, this, "e", 'f', (byte) 7, (short) 8);
        r += s.describe().length();
        r += list.size();
        r += super.hashCode();
        r += make(3).length();
        r += privateCall();
        r += Shape.unit().area() > 0 ? 1 : 0;
        return r;
    }

    private int privateCall() {
        return instanceInt;
    }

    // Lambdas, method references, inner, local, and anonymous classes.
    static int lambdas(int seed) {
        IntBinaryOperator add = (x, y) -> x + y + seed;
        IntBinaryOperator max = Math::max;
        Supplier<Sample> make = Sample::new;
        class Local {
            int twice(int v) {
                return 2 * v;
            }
        }
        Runnable r = new Runnable() {
            @Override
            public void run() {
                counter += seed;
            }
        };
        r.run();
        return add.applyAsInt(1, 2) + max.applyAsInt(3, 4) + new Local().twice(seed)
            + (make.get() == null ? 0 : 1);
    }

    // invoke-polymorphic (min-api 26 and up).
    static int handles(MethodHandle h, VarHandle v, Object[] arr) throws Throwable {
        int r = (int) h.invokeExact(3, "x");
        Object o = h.invoke(arr);
        v.set(arr, 0, o);
        return r + (int) v.get(arr, 1);
    }

    static MethodHandle lookup() throws ReflectiveOperationException {
        return MethodHandles.lookup().findStatic(Sample.class, "packed",
            MethodType.methodType(int.class, int.class));
    }

    // Locals in scopes that end and restart, of generic types.
    static <K extends Comparable<K>, V> int locals(Map<K, List<V>> map, K key) {
        int total = 0;
        for (Map.Entry<K, List<V>> e : map.entrySet()) {
            List<V> values = e.getValue();
            if (e.getKey().compareTo(key) > 0) {
                long wide = values.size();
                total += (int) wide;
            } else {
                double other = values.hashCode();
                total -= (int) other;
            }
        }
        {
            String s = "scope";
            total += s.length();
        }
        {
            Object s = new Object();
            total += s.hashCode();
        }
        return total;
    }

    // Static nested, inner, generic, and interface members.
    static class Nested<E> extends ArrayList<E> {
        @Override
        public boolean add(E e) {
            return super.add(e);
        }
    }

    class Inner {
        int outer() {
            return instanceInt;
        }
    }

    public interface Visitor<R> {
        R visit(Sample s);

        default R visitAll(Sample... all) {
            R r = null;
            for (Sample s : all) {
                r = visit(s);
            }
            return r;
        }

        static <R> Visitor<R> constant(R r) {
            return s -> r;
        }
    }

    @Deprecated
    public static void deprecated() {}

    @SafeVarargs
    public static <T> List<T> varargs(T... items) {
        return java.util.Arrays.asList(items);
    }

    public static String unicode() {
        return "\u00e9\u4e2d\ud83d\ude00\u0000end \"q\" \\ \n\r\t'";
    }

    public static void main(String[] args) throws Throwable {
        System.out.println(ints(1, 2) + longs(3, 4, 5) + floats(1, 2) + constants()
            + conversions(1, 2, 3, 4) + branches(1, 2, null) + packed(12) + sparse(77)
            + strings("beta") + colors(Color.RED) + arrays(3) + exceptions("12")
            + lambdas(4) + unicode() + STR + SURROGATES + CONTROLS + computed);
    }
}
