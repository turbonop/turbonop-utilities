// The C++ translation unit: the same COMDATs as mixed-c.c, from cl's C++
// front end.
#include "mixed.h"

extern "C" double mixed_cpp(double x) { return x * 1.25 + (double)mixed_env; }
