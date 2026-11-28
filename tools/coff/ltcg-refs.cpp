// /GL (LTCG) IL references where their compressed fields take more than
// one byte: lib.exe indexes an inline function only if something it
// indexes refers to it, so each field read wrong loses names from the
// index (as `use_shared` of tools/turbolib/incremental lost 14).
//
// - `wide`: refers to 150 inline functions (a count past 0x7F);
// - `hammer`: names `counter` 200 times (a use count past 0x7F), then
//   calls `after_hammer`;
// - `long_body`: a body whose length field passes 0x7F, then calls
//   `after_long`.
#define SHARED __declspec(noinline) inline

#define LEAF(n) SHARED int leaf##n(int x) { return x * n + 1; }
#define LEAVES(t) LEAF(t##0) LEAF(t##1) LEAF(t##2) LEAF(t##3) LEAF(t##4) \
    LEAF(t##5) LEAF(t##6) LEAF(t##7) LEAF(t##8) LEAF(t##9)
LEAVES(1) LEAVES(2) LEAVES(3) LEAVES(4) LEAVES(5) LEAVES(6) LEAVES(7) LEAVES(8)
LEAVES(9) LEAVES(10) LEAVES(11) LEAVES(12) LEAVES(13) LEAVES(14) LEAVES(15)

#define CALL(n) + leaf##n(x)
#define CALLS(t) CALL(t##0) CALL(t##1) CALL(t##2) CALL(t##3) CALL(t##4) \
    CALL(t##5) CALL(t##6) CALL(t##7) CALL(t##8) CALL(t##9)
int wide(int x) {
    return 0 CALLS(1) CALLS(2) CALLS(3) CALLS(4) CALLS(5) CALLS(6) CALLS(7) CALLS(8)
        CALLS(9) CALLS(10) CALLS(11) CALLS(12) CALLS(13) CALLS(14) CALLS(15);
}

int counter;
SHARED int after_hammer(int x) { return x - 1; }
#define TWICE(s) s s
#define HIT counter = counter * 3 + x;
SHARED int hammer(int x) {
    // 100 statements, each naming counter twice.
    TWICE(TWICE(TWICE(TWICE(TWICE(HIT HIT HIT)))))
    TWICE(TWICE(HIT))
    return after_hammer(counter);
}
int use_hammer(int x) { return hammer(x); }

SHARED int after_long(int x) { return x + 2; }
#define STEP x = x * 7 + (x >> 3);
SHARED int long_body(int x) {
    TWICE(TWICE(TWICE(TWICE(TWICE(STEP STEP)))))
    return after_long(x);
}
int use_long(int x) { return long_body(x); }
