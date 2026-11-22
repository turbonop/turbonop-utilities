/* Code that reaches static data through MIPS's GOT (see build.cmd): local
   GOT entries, which the loader relocates by its load address. */
static int hidden[4] = {1, 2, 3, 4};
static int counter;
int *pick(int i) {
    counter += i;
    return &hidden[i & 3];
}
