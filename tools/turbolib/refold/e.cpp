#define VARIANT 2
#include "../loose/shared.h"
#include "refold.h"

int e_run() {
    int sum = 0;
    for (int i = -2; i < 5; ++i) {
        sum += tripled(i) + both(i) + scaled(i, 5) + static_cast<int>(scaled(2.5, i));
    }
    Square s(4);
    const Shape& shape = s;
    return sum + shape.area() + counted() * 10;
}

const char* e_greeting() { return greeting(); }
int e_variant() { return variant() + selected; }
