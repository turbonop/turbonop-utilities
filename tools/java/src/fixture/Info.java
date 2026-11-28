package fixture;

import java.lang.annotation.ElementType;
import java.lang.annotation.Retention;
import java.lang.annotation.RetentionPolicy;
import java.lang.annotation.Target;

/** A runtime-visible annotation with a default of every element kind. */
@Retention(RetentionPolicy.RUNTIME)
@Target({ElementType.TYPE, ElementType.METHOD, ElementType.FIELD, ElementType.PARAMETER,
    ElementType.RECORD_COMPONENT, ElementType.CONSTRUCTOR, ElementType.LOCAL_VARIABLE})
public @interface Info {
    int id();
    byte b() default 1;
    char c() default 'c';
    double d() default 1.25;
    float f() default 2.5f;
    long j() default 1L << 40;
    short s() default -3;
    boolean z() default true;
    String name() default "default \"name\"";
    ElementType kind() default ElementType.TYPE;
    Class<?> type() default Object.class;
    Class<?> prim() default void.class;
    Class<?> arr() default String[][].class;
    Hidden nested() default @Hidden;
    int[] values() default {1, 2, 3};
    String[] empty() default {};
    Hidden[] many() default {@Hidden("a"), @Hidden("b")};
}
