/* 32-bit ARM (Thumb-2) code's references, for the cross-reference graph:
   calls, a table of function pointers (its address in a literal pool, or
   in MOVW and MOVT), data read and written, a function nothing reaches,
   and two functions of identical code. Freestanding: built for Linux (armhf, ELF with DWARF) and
   Windows on ARM (PE with a PDB) by tools/arm32/build.cmd. */

int counter;
const char greeting[] = "hello from 32-bit ARM";

__attribute__((noinline)) int square(int x) { return x * x; }
__attribute__((noinline)) int twice(int x) { return x + x; }
__attribute__((noinline)) int unused(int x) { return x - 7; }

/* Identical code, in two functions (which identical-code folding would
   make one). */
__attribute__((noinline)) int same_a(int x) { return x * 5 + 3; }
__attribute__((noinline)) int same_b(int x) { return x * 5 + 3; }

typedef int (*op)(int);
op const table[] = {square, twice};

/* (Volatile: the compiler can't fold the table's or the string's uses.) */
volatile int which;
const char *volatile where;

__attribute__((noinline)) int apply(int x) { return table[which](x); }

int start(void) {
    where = greeting;
    counter = apply(counter) + same_a(counter) + same_b(counter);
    return square(counter);
}
