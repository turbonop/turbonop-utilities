#include <stdio.h>
extern "C" int b_func(int); extern "C" int shared_f(int);
int main() { printf("b_func: %d\n", b_func(1)); printf("shared_f: %d\n", shared_f(1)); }
