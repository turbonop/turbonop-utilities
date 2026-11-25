#include <stdio.h>
extern "C" int b_func(int);
int main() { printf("b_func: %d\n", b_func(1)); }
