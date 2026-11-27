#include "pch.h"
struct Local {
  geo::Point p;
  int extra;
};
int use_u2(int v) {
  Local l{{v, v}, 3};
  return helper(l.p.x) * twice(l.extra) + counter();
}
