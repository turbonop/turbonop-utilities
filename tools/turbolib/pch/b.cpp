#include "shared.h"
int use_b(int v) { Point p{v, 3}; Box<short> b{(short)v}; return twice(p.sum()) + b.get(); }
