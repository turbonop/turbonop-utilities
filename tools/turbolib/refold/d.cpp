#define VARIANT 2
#include "../loose/shared.h"
#include "refold.h"

int d_run() {
    int sum = 0;
    for (int i = 0; i < 4; ++i) {
        sum += lookup(i) + doubled(i) + scaled(i, 7) + static_cast<int>(scaled(1.5, i));
    }
    Square s(3);
    const Shape& shape = s;
    return sum + shape.area() + counted();
}

const char* d_greeting() { return greeting(); }
const char* d_kind() {
    Square s(2);
    const Shape& shape = s;
    return shape.kind();
}
