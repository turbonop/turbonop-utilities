// The third translation unit of the library: it makes each of shared.h's
// functions do its work (a longjmp, a throw, a fault, calls through
// pointers), so that a missing table entry fails when the program runs.
#include "../../coff/tables/shared.h"

int exercise(int x) {
    int sum = 0;
    sum += add_address()(x, 3);           // a CFG-checked call to an inline function
    sum += twice_address()(x);            // and to a template
    sum += call(twice_address(), x);      // through an inline function
    sum += jumps(1);                      // longjmp (a .gljmp$y target)
    sum += throws(x + 1);                 // a catch's continuation (.gehcont$y)
#ifndef __clang__
    // An SEH handler (x86: .sxdata). (clang-cl's __try doesn't catch the
    // fault of a plain load without /EHa, so its programs fault here,
    // folded or not.)
    sum += faults(nullptr);
#endif
    sum += area_of(x);                    // virtual calls (a vftable's .gfids$y)
    volatile int v = x;
    sum += touch(&v);
    sum += pick(x % 7);
    unsigned long (__stdcall *tick)(void) = (unsigned long (__stdcall *)(void))tick_address();
    sum += tick() != 0;                   // a call through an import's address (.giats$y)
    return sum;
}
