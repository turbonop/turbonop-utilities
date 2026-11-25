#include "shared.h"
extern "C" int b_func(int v) { return shared_f(v) * 2; }
