int ec_data = 3;
int ec_func(int x) { return x + ec_data; }
static int helper(int y) { return y * 2; }
int ec_two(int z) { return helper(z); }
