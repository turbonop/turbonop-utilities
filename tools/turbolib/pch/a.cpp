#include "shared.h"
int use_a(int v) { Point p{v, 2}; Box<long> b{v}; return twice(p.sum()) + (int)b.get(); }
