// Compiled without /GL: a native object in an LTCG link.
int gl_plus_one(int x);

int native_twice(int x) { return 2 * gl_plus_one(x - 1) + x; }

int native_data[4] = {1, 2, 3, 4};
