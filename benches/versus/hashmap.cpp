#include <cstdio>
#include <cstdint>
#include <unordered_map>
int main() {
    uint64_t n = 2000000;
    std::unordered_map<uint64_t, uint64_t> table;
    for (uint64_t i = 0; i < n; i++) table[i * 2654435761ULL % 1000003 + i] = i;
    uint64_t sum = 0;
    for (uint64_t i = 0; i < n; i++) { auto it = table.find(i * 2654435761ULL % 1000003 + i); if (it != table.end()) sum += it->second; }
    std::printf("%llu\n%zu\n", (unsigned long long)sum, table.size());
}
