// A member with a dynamic initializer: loading it has a side effect.
#include "shared.h"

static int a_init = compute();

int a_value(int x) { return shared_add(x, twice(x)) + a_init + switch_on(x); }
