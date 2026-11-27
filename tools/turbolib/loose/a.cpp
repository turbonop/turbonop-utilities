#define VARIANT 1
#include "shared.h"

int a_run() {
    int sum = 0;
    for (int i = 0; i < 8; ++i) {
        sum += lookup(i) + doubled(i) + tripled(i) + both(i);
    }
    return sum + bump() + bump();
}

const char* a_greeting() { return greeting(); }
const char* a_name(int i) { return pick_name(i); }
int a_variant() { return variant() + selected + refers(); }
