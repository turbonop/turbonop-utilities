// What every object of the equivalence fixture's library shares, each a
// COMDAT that every object using it holds a copy of (so that folding moves
// them): switches with jump and index tables, inline thread_locals,
// lambdas, exceptions, RTTI, and two functions of identical code.
#pragma once

struct Err {
    int code;
};

// A dense switch: a jump table in the procedure, after its code.
inline int classify(int v) {
    switch (v) {
    case 0: return 11;
    case 1: return 23;
    case 2: return 37;
    case 3: return 41;
    case 4: return 53;
    case 5: return 67;
    case 6: return 71;
    case 7: return 83;
    default: return -1;
    }
}

// Many cases to few targets: a byte index table, then a jump table.
inline int sparse(int v) {
    switch (v) {
    case 0: case 4: case 9: case 13: case 17: case 22: return v * 2;
    case 1: case 5: case 10: case 14: case 18: case 23: return v + 100;
    case 2: case 6: case 11: case 15: case 19: case 24: return v - 7;
    case 3: case 7: case 12: case 16: case 20: case 25: return v ^ 0x55;
    default: return 0;
    }
}

// Thread-locals in COMDATs: folding moves them in the TLS template.
inline thread_local int tl_first = 3;
inline thread_local int tl_second = 5;

inline int tls_mix(int x) {
    tl_first += x;
    tl_second *= 2;
    return tl_first - tl_second;
}

// Two functions of the same code (nothing folds them in a Debug link).
inline int twin_a(int x) { return x * 7 + 1; }
inline int twin_b(int x) { return x * 7 + 1; }

inline int with_lambda(int x) {
    auto f = [x](int y) { return x * y + twin_a(y); };
    return f(x + 1);
}

struct Pair {
    int a, b;
};
inline int sum(const Pair& p) { return p.a + p.b; }

struct Guard {
    int* p;
    ~Guard() { ++*p; }
};

// FH4's tables (with unaligned RVAs), a destructor to unwind, a catch.
inline int risky(int x) {
    int n = 0;
    try {
        Guard g{&n};
        if (x > 5)
            throw Err{x};
        return x + sum(Pair{x, n});
    } catch (const Err& e) {
        return e.code + n;
    }
}

struct Base {
    virtual ~Base() {}
    virtual int f() const { return 1; }
};
struct Derived : Base {
    int v;
    explicit Derived(int x) : v(x) {}
    int f() const override { return v * 3; }
};

// RTTI: a dynamic_cast reads the complete object locator's RVAs.
inline int rtti(int x) {
    Derived d(x);
    const Base& b = d;
    const Derived* p = dynamic_cast<const Derived*>(&b);
    return p ? p->f() : 0;
}

inline const char* greeting() { return "hello from a shared inline function"; }

inline int shared_all(int x) {
    return classify(x & 7) + sparse(x % 27) + tls_mix(x) + twin_b(x) + with_lambda(x) +
           risky(x) + rtti(x);
}
