#include <cstdio>
#include <cstdint>
int main() {
    uint64_t limit = 3000000, best = 0, best_start = 0;
    for (uint64_t start = 1; start < limit; start++) {
        uint64_t x = start, steps = 0;
        while (x != 1) { if (x % 2 == 0) x /= 2; else x = 3 * x + 1; steps++; }
        if (steps > best) { best = steps; best_start = start; }
    }
    std::printf("%llu\n", (unsigned long long)best_start);
}
