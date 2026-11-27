// A static library of /GL objects (lib /LTCG): members pulled in by need.
int lib_used(int x) { return x ^ 0x55; }

int lib_also_used(int x) { return x - 7; }
