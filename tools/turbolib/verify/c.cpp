#include "shared.h"

static int c_local = 3;

int c_value(int x) {
    c_local += x;
    return shared_add(x, c_local) + twice(1.5) > 2 ? greeting()[x & 7] : shared_counter;
}
