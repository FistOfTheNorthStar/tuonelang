static long long lookup(const long long table[8], long long i) { return table[i]; }

static long long scan(const long long table[8]) {
    long long total = 0;
    for (int k = 0; k < 8; k++) {
        total += table[k];
    }
    return total;
}

static long long probe(const long long table[8]) {
    return lookup(table, 0) + lookup(table, 3) + lookup(table, 7);
}

int main(void) {
    long long table[8] = {3, 1, 4, 1, 5, 9, 2, 6};
    long long total = 0;
    for (long long round = 0; round < 200; round++) {
        total += scan(table) + probe(table);
    }
    return (int)total;
}
