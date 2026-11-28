package fixture;

import java.lang.annotation.Retention;
import java.lang.annotation.RetentionPolicy;

/** A class-retention annotation: RuntimeInvisibleAnnotations. */
@Retention(RetentionPolicy.CLASS)
public @interface Hidden {
    String value() default "";
}
