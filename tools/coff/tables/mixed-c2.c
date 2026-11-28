/* A second C translation unit, so that the C copies fold among themselves
   too. */
#include "mixed.h"

double mixed_c2(double x) { return x * 1.25 - (double)mixed_env; }
