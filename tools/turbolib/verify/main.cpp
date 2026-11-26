// The fixture program: no C runtime, so that it is small. It runs its own
// initializers (between __xc_a and __xc_z, as the CRT names them), has its
// own load configuration for /guard:cf, imports from kernel32, and exports
// a function.
#include "shared.h"

typedef void(__cdecl* PVFV)();
#pragma section(".CRT$XCA", read)
#pragma section(".CRT$XCZ", read)
extern "C" __declspec(allocate(".CRT$XCA")) PVFV __xc_a[] = {nullptr};
extern "C" __declspec(allocate(".CRT$XCZ")) PVFV __xc_z[] = {nullptr};

extern "C" __declspec(dllimport) unsigned long __stdcall GetTickCount();

// /guard:cf's check and dispatch functions, and the load configuration
// that names the linker's tables (as the CRT's loadcfg.c does).
extern "C" void __fastcall check_icall(void*) {}
#pragma section(".00cfg", read)
extern "C" __declspec(allocate(".00cfg")) void* __guard_check_icall_fptr = (void*)&check_icall;
extern "C" __declspec(allocate(".00cfg")) void* __guard_dispatch_icall_fptr = nullptr;
extern "C" unsigned int load_config_size();

static int (*const table[])(int) = {a_value, b_value, c_value};

extern "C" __declspec(dllexport) int fixture_export(int x) { return table[x % 3](x); }

extern "C" int entry() {
    for (PVFV* p = __xc_a + 1; p < __xc_z; ++p) {
        if (*p) {
            (*p)();
        }
    }
    int (*volatile f)(int) = fixture_export;
    return f(static_cast<int>(GetTickCount() & 7)) + load_config_size();
}
