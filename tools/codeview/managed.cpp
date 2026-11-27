// C++/CLI (/clr): managed code's CodeView, with metadata tokens.
// build.cmd compiles it.

ref class Counter {
public:
    int value;
    int Bump(int by) {
        value += by;
        return value;
    }
};

int run_managed(int n) {
    Counter^ c = gcnew Counter();
    int total = 0;
    for (int i = 0; i < n; ++i) {
        total += c->Bump(i);
    }
    return total;
}
