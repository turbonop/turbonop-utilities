// Several translation units, all /GL: calls across them, for LTCG to inline.
int multi_add(int a, int b) { return a + b; }

int multi_unused(int a) { return a * 3; }

int multi_counter;

void multi_bump(void) { ++multi_counter; }
