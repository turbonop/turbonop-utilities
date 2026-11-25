#include <stdio.h>
extern "C" int shared_f_tl(int v) { printf("X copy\n"); return v + 1; }
