#define VARIANT 2
#include "shared.h"

int b_run() {
    int sum = 0;
    for (int i = 0; i < 5; ++i) {
        sum += lookup(i * 3) + doubled(i) * tripled(i) + both(i + 1);
    }
    return sum + bump();
}

const char* b_greeting() { return greeting(); }
const char* b_name(int i) { return pick_name(i + 1); }
int b_variant() { return variant() + selected + refers(); }
