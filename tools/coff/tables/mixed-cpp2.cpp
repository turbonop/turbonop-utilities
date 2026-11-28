// A second C++ translation unit, so that the C++ copies fold among
// themselves too.
#include "mixed.h"

extern "C" double mixed_cpp2(double x) { return x * 1.25 - (double)mixed_env; }
