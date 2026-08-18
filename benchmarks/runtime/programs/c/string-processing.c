static const char line_text[] = "GET /users/42 HTTP/1.1 200 1532";
static const int line_len = (int)(sizeof line_text - 1);

static int count_byte(const char *text, int len, int target) {
    int found = 0;
    for (int i = 0; i < len; i++) {
        if ((unsigned char)text[i] == target) {
            found++;
        }
    }
    return found;
}

static int count_digits(const char *text, int len) {
    int found = 0;
    for (int i = 0; i < len; i++) {
        unsigned char byte = (unsigned char)text[i];
        if (byte >= 48 && byte <= 57) {
            found++;
        }
    }
    return found;
}

static int slice_equals(const char *text, int start, int end, const char *expect) {
    for (int i = start; i < end; i++) {
        if (text[i] != expect[i - start]) {
            return 0;
        }
    }
    return expect[end - start] == '\0';
}

static int score(const char *text, int len) {
    return count_byte(text, len, 32) + count_byte(text, len, 47) +
           count_digits(text, len) + slice_equals(text, 0, 3, "GET") +
           slice_equals(text, 14, 22, "HTTP/1.1");
}

int main(void) {
    int total = 0;
    for (int round = 0; round < 200; round++) {
        total += score(line_text, line_len);
    }
    return total;
}
