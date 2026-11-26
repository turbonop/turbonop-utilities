// The first translation unit: its own functions, and copies of shared.h's.
#include "shared.h"

static int seven(int x) { return x + 7; }

unary one_static() { return &seven; }
int one(int x) {
    volatile int v = x;
    return add_address()(x, 1) + twice_address()(x) + (tick_address() != 0) + jumps(0) +
           throws(x) + area_of(x) + touch(&v) + call(&twice<int>, x) + pick(x) + faults(&x);
}
