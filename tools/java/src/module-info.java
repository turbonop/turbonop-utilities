/** The fixtures' module. */
@Deprecated
module fixture.mod {
    requires java.logging;
    requires transitive java.sql;
    requires static java.compiler;
    exports fixture;
    exports fixture.internal to java.logging;
    opens fixture.internal;
    uses java.lang.Runnable;
    provides java.lang.Runnable with fixture.internal.Impl;
}
