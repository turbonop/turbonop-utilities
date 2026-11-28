// Writes dex/sample/Indy.class by hand: what javac can't be made to emit from
// Java source, but d8 carries into DEX as it is (from --min-api 28):
// invokedynamic with a bootstrap method of our own (DEX call sites, whose
// static arguments are every kind of constant: invoke-custom), and ldc of
// method handles of every kind and of method types (const-method-handle,
// const-method-type). The methods are straight-line code, so the class
// needs no StackMapTable.
//
//   java MakeIndy.java <output directory>

import java.io.ByteArrayOutputStream;
import java.io.DataOutputStream;
import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.HashMap;
import java.util.Map;

public class MakeIndy {
    private final ByteArrayOutputStream poolBytes = new ByteArrayOutputStream();
    private final DataOutputStream pool = new DataOutputStream(poolBytes);
    private final Map<String, Integer> entries = new HashMap<>();
    private int count = 1;

    private int entry(String key, int slots, IoAction write) throws IOException {
        Integer index = entries.get(key);
        if (index != null) {
            return index;
        }
        write.run();
        int i = count;
        count += slots;
        entries.put(key, i);
        return i;
    }

    interface IoAction {
        void run() throws IOException;
    }

    int utf8(String s) throws IOException {
        return entry("U" + s, 1, () -> {
            pool.writeByte(1);
            pool.writeUTF(s);
        });
    }

    int integer(int v) throws IOException {
        return entry("I" + v, 1, () -> {
            pool.writeByte(3);
            pool.writeInt(v);
        });
    }

    int floating(float v) throws IOException {
        return entry("F" + v, 1, () -> {
            pool.writeByte(4);
            pool.writeFloat(v);
        });
    }

    int wide(long v) throws IOException {
        return entry("J" + v, 2, () -> {
            pool.writeByte(5);
            pool.writeLong(v);
        });
    }

    int dbl(double v) throws IOException {
        return entry("D" + v, 2, () -> {
            pool.writeByte(6);
            pool.writeDouble(v);
        });
    }

    int type(String internal) throws IOException {
        int name = utf8(internal);
        return entry("C" + internal, 1, () -> {
            pool.writeByte(7);
            pool.writeShort(name);
        });
    }

    int string(String s) throws IOException {
        int u = utf8(s);
        return entry("S" + s, 1, () -> {
            pool.writeByte(8);
            pool.writeShort(u);
        });
    }

    int nameAndType(String name, String desc) throws IOException {
        int n = utf8(name);
        int d = utf8(desc);
        return entry("N" + name + ":" + desc, 1, () -> {
            pool.writeByte(12);
            pool.writeShort(n);
            pool.writeShort(d);
        });
    }

    /** A Fieldref (9), Methodref (10), or InterfaceMethodref (11). */
    int member(int tag, String owner, String name, String desc) throws IOException {
        int c = type(owner);
        int nt = nameAndType(name, desc);
        return entry("M" + tag + owner + "." + name + ":" + desc, 1, () -> {
            pool.writeByte(tag);
            pool.writeShort(c);
            pool.writeShort(nt);
        });
    }

    int methodHandle(int kind, int ref) throws IOException {
        return entry("H" + kind + ":" + ref, 1, () -> {
            pool.writeByte(15);
            pool.writeByte(kind);
            pool.writeShort(ref);
        });
    }

    int methodType(String desc) throws IOException {
        int d = utf8(desc);
        return entry("T" + desc, 1, () -> {
            pool.writeByte(16);
            pool.writeShort(d);
        });
    }

    int invokeDynamic(int bootstrap, String name, String desc) throws IOException {
        int nt = nameAndType(name, desc);
        return entry("Y" + bootstrap + name + desc, 1, () -> {
            pool.writeByte(18);
            pool.writeShort(bootstrap);
            pool.writeShort(nt);
        });
    }

    static final String BOOT = "dex/sample/Bootstraps";

    byte[] build() throws IOException {
        int self = type("dex/sample/Indy");
        int object = type("java/lang/Object");
        int code = utf8("Code");
        int bootstrapAttr = utf8("BootstrapMethods");
        int sourceFile = utf8("SourceFile");
        int sourceName = utf8("MakeIndy.java");

        // The bootstrap method, and its static arguments: one of each kind
        // of loadable constant.
        int bsm = methodHandle(6, member(10, BOOT, "bootstrap",
            "(Ljava/lang/invoke/MethodHandles$Lookup;Ljava/lang/String;"
                + "Ljava/lang/invoke/MethodType;[Ljava/lang/Object;)Ljava/lang/invoke/CallSite;"));
        int[] args0 = {
            string("text"), integer(42), wide(1L << 40), floating(1.5f), dbl(2.25),
            type("java/lang/String"), methodType("(I)J"),
            methodHandle(6, member(10, BOOT, "target", "()Ljava/lang/Object;")),
        };
        int[] args1 = {integer(-1), string("second")};

        // The method handles, one of each kind (JVMS 5.4.3.5).
        int[] handles = {
            methodHandle(1, member(9, BOOT, "instanceField", "J")),
            methodHandle(2, member(9, BOOT, "staticField", "I")),
            methodHandle(3, member(9, BOOT, "instanceField", "J")),
            methodHandle(4, member(9, BOOT, "staticField", "I")),
            methodHandle(5, member(10, BOOT, "run", "()V")),
            methodHandle(6, member(10, BOOT, "target", "(ILjava/lang/String;)J")),
            methodHandle(7, member(10, "dex/sample/Indy", "own", "()V")),
            methodHandle(8, member(10, BOOT, "<init>", "()V")),
            methodHandle(9, member(11, "java/lang/Runnable", "run", "()V")),
        };
        int type = methodType("(Ljava/lang/String;[I)Ljava/lang/Object;");

        int site0 = invokeDynamic(0, "target", "()Ljava/lang/Object;");
        int site1 = invokeDynamic(1, "target", "(ILjava/lang/String;)J");

        ByteArrayOutputStream methodsBytes = new ByteArrayOutputStream();
        DataOutputStream methods = new DataOutputStream(methodsBytes);
        int methodCount = 0;

        // public static Object site() { return indy target()Object; }
        {
            ByteArrayOutputStream c = new ByteArrayOutputStream();
            DataOutputStream o = new DataOutputStream(c);
            o.writeByte(0xba);
            o.writeShort(site0);
            o.writeShort(0);
            o.writeByte(0xb0); // areturn
            method(methods, 0x0009, "site", "()Ljava/lang/Object;", code, 1, 0, c.toByteArray());
            methodCount++;
        }
        // public static long site2(int, String) { return indy target(I, String)J; }
        {
            ByteArrayOutputStream c = new ByteArrayOutputStream();
            DataOutputStream o = new DataOutputStream(c);
            o.writeByte(0x1a); // iload_0
            o.writeByte(0x2b); // aload_1
            o.writeByte(0xba);
            o.writeShort(site1);
            o.writeShort(0);
            o.writeByte(0xad); // lreturn
            method(methods, 0x0009, "site2", "(ILjava/lang/String;)J", code, 2, 2,
                c.toByteArray());
            methodCount++;
        }
        // public static Object handles() { ldc each handle; pop; ...; ldc type; areturn }
        {
            ByteArrayOutputStream c = new ByteArrayOutputStream();
            DataOutputStream o = new DataOutputStream(c);
            for (int h : handles) {
                o.writeByte(0x13); // ldc_w
                o.writeShort(h);
                o.writeByte(0x57); // pop
            }
            o.writeByte(0x13);
            o.writeShort(type);
            o.writeByte(0xb0);
            method(methods, 0x0009, "handles", "()Ljava/lang/Object;", code, 1, 0,
                c.toByteArray());
            methodCount++;
        }
        // private void own() {}
        {
            method(methods, 0x0002, "own", "()V", code, 0, 1, new byte[] {(byte) 0xb1});
            methodCount++;
        }

        ByteArrayOutputStream out = new ByteArrayOutputStream();
        DataOutputStream f = new DataOutputStream(out);
        f.writeInt(0xCAFEBABE);
        f.writeShort(0);
        f.writeShort(52);
        f.writeShort(count);
        f.write(poolBytes.toByteArray());
        f.writeShort(0x0021); // public super
        f.writeShort(self);
        f.writeShort(object);
        f.writeShort(0); // interfaces
        f.writeShort(0); // fields
        f.writeShort(methodCount);
        f.write(methodsBytes.toByteArray());
        f.writeShort(2); // attributes
        f.writeShort(sourceFile);
        f.writeInt(2);
        f.writeShort(sourceName);
        f.writeShort(bootstrapAttr);
        int[][] all = {args0, args1};
        int length = 2;
        for (int[] a : all) {
            length += 4 + 2 * a.length;
        }
        f.writeInt(length);
        f.writeShort(all.length);
        for (int[] a : all) {
            f.writeShort(bsm);
            f.writeShort(a.length);
            for (int x : a) {
                f.writeShort(x);
            }
        }
        return out.toByteArray();
    }

    void method(DataOutputStream m, int access, String name, String desc, int code, int stack,
            int locals, byte[] bytecode) throws IOException {
        m.writeShort(access);
        m.writeShort(utf8(name));
        m.writeShort(utf8(desc));
        m.writeShort(1);
        m.writeShort(code);
        m.writeInt(12 + bytecode.length);
        m.writeShort(stack);
        m.writeShort(locals);
        m.writeInt(bytecode.length);
        m.write(bytecode);
        m.writeShort(0); // exceptions
        m.writeShort(0); // attributes
    }

    public static void main(String[] args) throws IOException {
        MakeIndy w = new MakeIndy();
        // (The pool is complete once the methods are written: build twice,
        // the second time with every entry already there, so that the
        // pool written out holds the methods' names too.)
        w.build();
        byte[] bytes = w.build();
        Path dir = Path.of(args[0], "dex", "sample");
        Files.createDirectories(dir);
        Files.write(dir.resolve("Indy.class"), bytes);
    }
}
