// Only b.cpp includes this, first: its objects' files are in another order.
#pragma once
struct Other {
  long long wide;
  double ratio;
};
inline double other_ratio(const Other &o) { return o.ratio * 2; }
