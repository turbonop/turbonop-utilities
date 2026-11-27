// Uses dll.c's DLL through its import library: by name, its datum, a
// function exported under another name, and one by ordinal only.
#include <stdio.h>
#include <windows.h>
__declspec(dllimport) int add(int, int);
__declspec(dllimport) extern int counter;
int times(int, int);
int main(void) {
    HMODULE m = GetModuleHandleA("dll.dll");
    int (*hidden)(void) = (int (*)(void))GetProcAddress(m, MAKEINTRESOURCEA(9));
    printf("add=%d counter=%d times=%d hidden=%d\n", add(2, 3), counter, times(6, 7), hidden ? hidden() : -1);
    return 0;
}
