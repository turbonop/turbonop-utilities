// A member of the equivalence fixture's library: shared.h's COMDATs, and
// a thread-local and a switch of its own.
#include "shared.h"

static thread_local int tl_a = 1;

static int own_a(int v) {
    switch (v) {
    case 0: return 5;
    case 1: return 9;
    case 2: return 14;
    case 3: return 20;
    case 4: return 27;
    default: return v;
    }
}

int a_value(int x) {
    tl_a += x;
    return shared_all(x) + own_a(x % 6) + tl_a + (greeting()[x % 8]);
}
