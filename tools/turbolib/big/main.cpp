#include <stdio.h>
extern "C" const char big_5[];
int main() { printf("%d\n", big_5[12345]); }
