/* A PE image with DWARF, as MinGW and clang's windows-gnu targets build
   one: no CRT (it starts at start), code and data in their own sections. */

static int helper(int x) { return x * 3 + 1; }

__attribute__((noinline)) int used(int x) { return helper(x) + 2; }

/* Kept: nothing calls it, but nothing collects it either (no --gc-sections). */
__attribute__((noinline)) int unused(int x) { return x - 7; }

const char message[] = "hello from a PE image with DWARF";

int table[4] = {1, 2, 3, 4};

int start(void) { return used((int)message[0]) + table[1]; }
