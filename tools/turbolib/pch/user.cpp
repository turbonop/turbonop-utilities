#include "shared.h"
int use_user(int v) { Point p{v, 1}; Box<long> b{v}; return twice(p.sum()) + (int)b.get(); }
