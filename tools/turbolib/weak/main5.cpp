#include <stdio.h>
extern "C" int a_func(int);
int main() { printf("a_func: %d\n", a_func(1)); }
