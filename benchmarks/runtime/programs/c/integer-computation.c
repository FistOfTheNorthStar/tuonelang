static int sum(int n, int acc) {
    return n == 0 ? acc : sum(n - 1, acc + n);
}
int main(void) { return sum(1000, 0); }
