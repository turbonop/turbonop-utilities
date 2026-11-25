/* COFF symbols of every kind clang writes: weak definitions and weak
   references (weak externals, with their defaults), a static, a file's
   name (.file), and, for MSVC's target, COMDATs with their associative
   debug sections. */

__attribute__((weak)) int maybe(int x) { return x + 1; }

extern int other(int) __attribute__((weak));

static int counter;

int use(int x) {
    counter++;
    return maybe(x) + (other ? other(x) : 0);
}
