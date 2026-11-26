#include "other.h"
#include "shared.h"
int use_b(int v) {
  Other o{v, 0.5};
  return twice(v) * helper(v) + static_cast<int>(twice(2.5) + other_ratio(o)) + counter();
}
