/* A module's data, as clang and wasm-ld lay it out in linear memory: its
   read-only data (.rodata: a table, strings, a duplicated array), its
   writable data (.data), and zeroed data (.bss), each a variable DWARF
   describes; an exported data address; and code that reads them. */

const int primes[8] = {2, 3, 5, 7, 11, 13, 17, 19};

/* The same bytes twice: one's a duplicate. */
const unsigned char twice_a[64] = {1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16};
const unsigned char twice_b[64] = {1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16};

const char *const greeting = "hello from linear memory";

int counter = 42;

int scratch[16];

__attribute__((export_name("lookup"))) int lookup(int i) {
    counter++;
    scratch[i & 15] = primes[i & 7];
    return twice_a[i & 63] + twice_b[(i + 1) & 63] + greeting[i & 7] + scratch[0];
}
