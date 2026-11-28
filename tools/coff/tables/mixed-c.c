/* The C translation unit (cl /TC): mixed.h's COMDATs, and a floating-point
   constant (__real@...) of its own copy. */
#include "mixed.h"

double mixed_c(double x) { return x * 1.25 + (double)mixed_env; }
