#include <cstdio>
#include <cstdint>
static uint64_t fib(uint64_t n) { if (n < 2) return n; return fib(n - 1) + fib(n - 2); }
int main() { std::printf("%llu\n", (unsigned long long)fib(40)); }
