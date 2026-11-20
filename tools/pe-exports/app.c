/* Imports two of exp.dll's exports. */

__declspec(dllimport) int exported_function_number_00(int x);
__declspec(dllimport) int exported_function_number_01(int x);

int start(void) { return exported_function_number_00(1) + exported_function_number_01(2); }
