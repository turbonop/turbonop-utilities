/* A library the program is linked with but calls nothing of:
   libneeded-unused.so (or .dylib), which --as-needed (or
   -dead_strip_dylibs) would leave out. */

int unused_fn(int x) { return x * 2; }
