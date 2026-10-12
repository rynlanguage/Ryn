#include <cstdio>
#include <cstdlib>
int main() {
    size_t count = 1048576;
    float *data = (float*)malloc(count * 4);
    for (size_t at = 0; at < count; at++) data[at] = (float)(at % 7) * 0.5f;
    float total = 0.0f;
    for (int pass = 0; pass < 64; pass++) {
        float partial = 0.0f;
        for (size_t i = 0; i < count; i++) partial += data[i];
        total += partial;
    }
    std::printf("%g\n", total);
    free(data);
}
