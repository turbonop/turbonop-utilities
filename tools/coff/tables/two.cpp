// The second translation unit: its own functions, and copies of shared.h's.
#include "shared.h"

static int nine(int x) { return x + 9; }

unary two_static() { return &nine; }
int two(int x) {
    volatile int v = x;
    return add_address()(x, 2) + twice_address()(x) + (tick_address() != 0) + jumps(0) +
           throws(x) + area_of(x) + touch(&v) + call(&nine, x) + pick(x) + faults(&x);
}
