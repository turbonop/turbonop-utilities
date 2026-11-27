// Uses the precompiled header (/Yu): its .debug$T starts with LF_PRECOMP,
// and its own types are numbered after the header's.
#include "pch.h"

struct Local {
    Shared s;
    Color c;
};

int use_shared(Local* l) {
    return shared_twice(l->s.a) + static_cast<int>(l->c);
}
