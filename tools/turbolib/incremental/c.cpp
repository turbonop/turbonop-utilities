// A library object without the precompiled header.
#include "shared.h"

int c_value() {
    Ring<int, 3> r;
    for (int i = 0; i < 5; ++i) r.push(shared_add(i, 2));
#ifdef EXTRA_C
    // (An experiment's change to this object alone.)
    return use_shared(3) + r.sum() + static_cast<int>(twice(2LL)) + 1;
#else
    return use_shared(3) + r.sum() + static_cast<int>(twice(2LL));
#endif
}
