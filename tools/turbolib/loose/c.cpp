#define VARIANT 2
#include "shared.h"

int ext_one() { return 1000; }
int ext_two() { return 2000; }

int c_run() {
    int sum = 0;
    for (int i = -3; i < 3; ++i) {
        sum += lookup(i) - doubled(i) + tripled(i) + both(i);
    }
    return sum + bump() * 7;
}

const char* c_greeting() { return greeting(); }
int c_variant() { return variant() + selected + refers(); }
