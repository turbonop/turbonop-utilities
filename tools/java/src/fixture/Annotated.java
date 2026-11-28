package fixture;

import java.io.IOException;
import java.io.Serializable;
import java.util.List;
import java.util.Map;
import java.util.function.Supplier;

/** Annotations everywhere they go, and type annotations of every target. */
@Info(id = 1, name = "annotated", kind = java.lang.annotation.ElementType.FIELD, type = String.class,
    values = {4, 5}, nested = @Hidden("n"))
@Hidden("class")
public class Annotated<@TypeUse("T") T extends @TypeUse("bound") Serializable & @HiddenType Comparable<T>>
        extends @TypeUse("super") Object implements @TypeUse("iface") Supplier<@TypeUse("arg") T> {
    @Info(id = 2) @Hidden
    public @TypeUse("field") String field;

    public Map<@TypeUse("key") String, List<@TypeUse("wild") ? extends @TypeUse("ext") Number>> nested;

    public String @TypeUse("array") [] @TypeUse("inner") [] arrays;

    public @TypeUse("outer") Annotated<T>.@TypeUse("in") Inner inner;

    public class Inner {}

    @Info(id = 3)
    public Annotated() {}

    public <@TypeUse("U") U extends @TypeUse("ubound") Number> @TypeUse("ret") U method(
            @Info(id = 4) @TypeUse("param") final U u, @Hidden("p") int plain,
            @TypeUse("varargs") String @TypeUse("dims") ... rest)
            throws @TypeUse("throws") IOException, @HiddenType RuntimeException {
        @TypeUse("local") Object local = u;
        Object cast = (@TypeUse("cast") Number) local;
        if (cast instanceof @TypeUse("instanceof") Integer) {
            local = new @TypeUse("new") StringBuilder();
        }
        try (@TypeUse("resource") AutoCloseable r = () -> {}) {
            local = List.<@TypeUse("typearg") String>of();
        } catch (@TypeUse("catch") Exception e) {
            throw new IOException(e);
        }
        Supplier<Object> s = @TypeUse("ref") StringBuilder::new;
        return u;
    }

    public void receiver(@TypeUse("this") Annotated<T> this) {}

    @Override
    public T get() {
        return null;
    }
}
