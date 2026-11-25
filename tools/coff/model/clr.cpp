// A C++/CLI object (see ..\model.cmd): its managed functions have CLR
// token symbols.
int managed(int n) { return n * 2; }
int calls_managed(int n) { return managed(n) + 1; }
