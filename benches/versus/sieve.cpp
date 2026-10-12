#include <cstdio>
#include <cstdint>
#include <vector>
int main() {
    uint64_t n = 20000000;
    std::vector<unsigned char> composite;
    for (uint64_t i = 0; i <= n; i++) composite.push_back(0);
    for (uint64_t p = 2; p * p <= n; p++)
        if (!composite[p]) for (uint64_t j = p * p; j <= n; j += p) composite[j] = 1;
    uint64_t count = 0;
    for (uint64_t k = 2; k <= n; k++) if (!composite[k]) count++;
    std::printf("%llu\n", (unsigned long long)count);
}
