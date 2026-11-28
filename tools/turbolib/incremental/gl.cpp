// A library object built with /GL (passed through by turbolib; the link
// is then an LTCG link).
#include "shared.h"

int gl_value() { return use_shared(5) + 2; }
