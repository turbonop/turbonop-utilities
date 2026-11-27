#include "shared.h"

int compute() { return shared_add(4, 5) + shared_counter; }

int b_value(int x) {
    Square s(x);
    const Shape& shape = s;
    return shape.area() + twice(x) + switch_on(x + 1);
}
