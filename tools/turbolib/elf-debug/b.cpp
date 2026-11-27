#include "shared.h"

int use_b(int k) {
  std::vector<int> v{k, -k, 2 * k};
  fx::Point p{k, 2};
  fx::Rect r(k, 3);
  const fx::Shape& s = r;
  int t = fx::accumulate(v, 1) + p.dot(fx::Point{1, k}) + s.area();
  t += fx::counter() + fx::with_lambda(k);
  try {
    t += fx::checked(k);
  } catch (const std::exception&) {
    t -= 1;
  }
  return t + static_cast<int>(fx::mix(k, t));
}
