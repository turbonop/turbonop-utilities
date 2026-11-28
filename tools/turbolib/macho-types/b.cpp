#include "types.h"
int b(int v) {
  Point p{v, v};
  return twice(p) + peek(reinterpret_cast<const Hidden *>(&v));
}
