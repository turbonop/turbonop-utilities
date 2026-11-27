// A DLL: a function and a datum exported by __declspec(dllexport), and a
// function exported by the .def file only (by ordinal, without a name, and
// under another name).
#include <windows.h>
__declspec(dllexport) int add(int a, int b) { return a + b; }
__declspec(dllexport) int counter = 40;
int times_impl(int a, int b) { return a * b; }
int hidden(void) { return 7; }
BOOL WINAPI DllMain(HINSTANCE h, DWORD reason, LPVOID r) { (void)h; (void)r; if (reason == DLL_PROCESS_ATTACH) counter += 1; return TRUE; }
