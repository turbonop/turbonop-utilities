#include "types.h"
int a(Point p) { return twice(p) + peek(reinterpret_cast<const Hidden *>(&p)); }
