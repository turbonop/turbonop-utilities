package fixture;

import java.lang.annotation.ElementType;
import java.lang.annotation.Target;

/** A type annotation (invisible). */
@Target({ElementType.TYPE_USE, ElementType.TYPE_PARAMETER})
public @interface HiddenType {}
