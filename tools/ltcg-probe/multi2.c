#include <stdio.h>

int multi_add(int a, int b);
void multi_bump(void);
extern int multi_counter;

int main(int argc, char** argv) {
    (void)argv;
    multi_bump();
    printf("%d %d\n", multi_add(argc, 2), multi_counter);
    return 0;
}
