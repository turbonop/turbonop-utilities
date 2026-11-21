/* A DLL that MinGW's linker keeps a COFF symbol table in, with exports. */

__declspec(dllexport) int add(int a, int b) { return a + b; }

static int helper(int x) { return x * 3; }

__declspec(dllexport) int triple(int x) { return helper(x); }

int DllMainCRTStartup(void *module, unsigned reason, void *reserved) {
    (void)module;
    (void)reason;
    (void)reserved;
    return 1;
}
