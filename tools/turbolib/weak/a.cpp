#include "shared.h"
struct Init { Init() { printf("A loaded\n"); } } init;
extern "C" int a_func(int v) { return shared_f(v); }
