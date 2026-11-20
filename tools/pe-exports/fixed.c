/* A program linked /FIXED (no base relocations): a table of function
   pointers in read-only data, whose words are the functions' addresses,
   with nothing to say they are. */

__attribute__((noinline)) static int one(int x) { return x + 1; }
__attribute__((noinline)) static int two(int x) { return x + 2; }

int (*const table[])(int) = {one, two};

/* (Through a volatile pointer to it: the compiler can't know what the
   table holds, so its words are what the code calls.) */
int (*const *volatile through)(int) = table;
volatile int which;

int start(void) { return through[which](3); }
