#include <iostream>
#include <string>
#include <vector>
#include <stdexcept>
#include <memory>
#include <map>
struct Shape { virtual ~Shape() = default; virtual double area() const = 0; };
struct Square : Shape { double s; explicit Square(double s) : s(s) {} double area() const override { return s * s; } };
static std::map<std::string, int> g_table = {{"one", 1}, {"two", 2}};
thread_local int t_count = 41;
int main(int argc, char** argv) {
    std::vector<std::unique_ptr<Shape>> v;
    for (int i = 1; i <= 3; ++i) v.push_back(std::make_unique<Square>(i));
    double total = 0; for (auto& s : v) total += s->area();
    try { if (argc > 0) throw std::runtime_error("boom " + std::to_string(total)); }
    catch (const std::exception& e) { std::cout << "caught: " << e.what() << "\n"; }
    ++t_count;
    std::cout << "table two=" << g_table["two"] << " tls=" << t_count << "\n";
    return static_cast<int>(total);
}
