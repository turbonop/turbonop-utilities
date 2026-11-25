#include <stdio.h>
extern "C" inline int shared_f(int v) { printf("AB copy\n"); return v + 1; }
