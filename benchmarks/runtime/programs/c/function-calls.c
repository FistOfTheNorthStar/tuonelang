static int f(int n) { return n + 1; }
static int g(int n) { return f(n) + f(n); }
int main(void) { return g(3) + g(4) + g(5); }
