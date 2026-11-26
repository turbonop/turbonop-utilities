/* An x86 object whose .drectve exports (/EXPORT:) lib /DEF takes. */
__declspec(dllexport) int __stdcall Func(int a, int b) { return a + b; }
__declspec(dllexport) int __cdecl Cd(int a) { return a; }
__declspec(dllexport) int __fastcall Fa(int a) { return a; }
__declspec(dllexport) int Data1 = 4;
int Plain(int a) { return a; }
#pragma comment(linker, "/EXPORT:Extra=Plain,@7")
#pragma comment(linker, "/EXPORT:Dd=Data1,DATA")
