#include <cstdio>
#include <cstdint>
struct Node { uint64_t value; Node* next; };
int main() {
    uint64_t n = 5000000;
    Node* head = nullptr;
    for (uint64_t i = 0; i < n; i++) { Node* node = new Node{i, head}; head = node; }
    uint64_t sum = 0;
    for (Node* at = head; at; at = at->next) sum += at->value;
    std::printf("%llu\n", (unsigned long long)sum);
    uint64_t freed = 0;
    while (head) { Node* next = head->next; delete head; head = next; freed++; }
    std::printf("%llu\n", (unsigned long long)(0 * freed));
}
