/* json-parse — the equivalent-semantics C peer for the tuonelang json-parse
 * workload (ADR-0016). The identical walk: per round, recursive-descent
 * parse of the same fixed document (numbers via strtod — the same decimal
 * accumulation), counting nodes and summing every number, then fold the
 * same structural checksum: (nodes * 3 + (long long)sum) % 256 = 54, the
 * exit byte (the last round's value, reassigned, not accumulated). */
#include <stdlib.h>

static const char *DOC =
    "{\"id\":42,\"name\":\"tuonelang\",\"tags\":[\"fast\",\"safe\",\"native\"],"
    "\"metrics\":{\"stars\":128,\"forks\":32,\"score\":9.5},\"active\":true,"
    "\"refs\":[1,2,3,4,5,6,7,8]}";

static const char *skip_ws(const char *p) {
    while (*p == ' ') p++;
    return p;
}

static const char *string_end(const char *p) {
    p++;
    while (*p && *p != '"') p++;
    return p + 1;
}

static const char *parse_value(const char *p, long long *nodes, double *sum);

static const char *parse_items(const char *p, char closer, long long *nodes,
                               double *sum) {
    p = skip_ws(p);
    if (*p == closer) return p + 1;
    for (;;) {
        char c;
        if (closer == '}') {
            p = skip_ws(p);
            p = string_end(p);
            p = skip_ws(p);
            p++; /* ':' */
        }
        p = parse_value(p, nodes, sum);
        p = skip_ws(p);
        c = *p;
        p++;
        if (c == closer) return p;
    }
}

static const char *parse_value(const char *p, long long *nodes, double *sum) {
    p = skip_ws(p);
    (*nodes)++;
    if (*p == 'n' || *p == 't') return p + 4;
    if (*p == 'f') return p + 5;
    if (*p == '"') return string_end(p);
    if (*p == '[') return parse_items(p + 1, ']', nodes, sum);
    if (*p == '{') return parse_items(p + 1, '}', nodes, sum);
    {
        char *end;
        *sum += strtod(p, &end);
        return end;
    }
}

static long long round_trip(void) {
    long long nodes = 0;
    double sum = 0.0;
    parse_value(DOC, &nodes, &sum);
    return (nodes * 3 + (long long)sum) % 256;
}

int main(void) {
    long long result = 0;
    long long r;
    for (r = 0; r < 200; r++) {
        result = round_trip();
    }
    return (int)(result & 0xff);
}
