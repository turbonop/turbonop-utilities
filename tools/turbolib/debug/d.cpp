// The headers in another order than c.cpp's.
#include "inl6.h"
#include "inl5.h"
#include "inl4.h"
#include "inl3.h"
#include "inl2.h"
#include "inl1.h"
#include "deep.h"
void use_d(int v) { deep(v * 2); sink(shallow(v + 1)); }
