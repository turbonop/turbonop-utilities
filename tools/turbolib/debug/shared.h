// Inline functions, templates, and types that every object of the fixture
// shares: each object holds its own copy, which turbolib folds.
#pragma once

namespace geo {
struct Point {
  int x, y;
  int sum() const { return x + y; }
  Point scaled(int k) const { return {x * k, y * k}; }
};
enum class Color : unsigned char { Red, Green, Blue };
template <class T> struct Box {
  T value;
  T get() const { return value; }
};
} // namespace geo

template <class T> T twice(T v) {
  T r = v;
  r = r + v;
  return r;
}

inline int helper(int a) {
  geo::Point p{a, a + 1};
  geo::Box<geo::Point> b{p.scaled(2)};
  return b.get().sum() * 3;
}

inline int counter() {
  static int n = 0;
  return ++n;
}

inline int answer = 42;

inline int color_value(geo::Color c) {
  switch (c) {
  case geo::Color::Red:
    return 1;
  case geo::Color::Green:
    return 2;
  default:
    return 3;
  }
}
