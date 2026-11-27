// A DLL with exports, C++ classes with virtual functions, templates,
// exceptions, and a static initializer: /GL, /EHsc, /DEBUG:FULL.
#include <stdexcept>
#include <string>
#include <vector>

struct Shape {
    virtual ~Shape() = default;
    virtual double area() const = 0;
    virtual std::string name() const { return "shape"; }
};

struct Rect : Shape {
    double w, h;
    Rect(double w_, double h_) : w(w_), h(h_) {}
    double area() const override { return w * h; }
    std::string name() const override { return "rect"; }
};

template <typename T>
T sum(const std::vector<T>& v) {
    T total{};
    for (const T& x : v) total += x;
    return total;
}

static std::vector<int> g_table = {1, 2, 3, 5, 8};

extern "C" __declspec(dllexport) int dll_sum() { return sum(g_table); }

extern "C" __declspec(dllexport) double dll_area(double w, double h) {
    if (w < 0) throw std::invalid_argument("negative");
    Rect r(w, h);
    const Shape& s = r;
    return s.area() + static_cast<double>(s.name().size());
}

__declspec(dllexport) int dll_cpp_export(const std::string& s) { return static_cast<int>(s.size()); }
