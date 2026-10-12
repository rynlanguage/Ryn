#!/usr/bin/env python3
"""Writes big.ryn, big.rs and big.cpp: 3000 small functions with a loop and a branch each, and a main that adds up their results."""
N = 3000
ryn, rs, cpp = [], [], ["#include <cstdio>\n#include <cstdint>"]
for i in range(N):
    k = i % 97 + 3
    ryn.append(f"""fun f{i}(x: u64) -> u64 {{
    mut a: u64 = x * {k} + {i}
    mut j: u64 = 0
    while j < 4 {{
        when a % {k + 2} == 0 {{
            a = a / 3 + j
        }} else {{
            a = a * 5 + {i % 11}
        }}
        j += 1
    }}
    return a + {i}
}}
""")
    rs.append(f"""fn f{i}(x: u64) -> u64 {{
    let mut a: u64 = x.wrapping_mul({k}).wrapping_add({i});
    let mut j: u64 = 0;
    while j < 4 {{
        if a % {k + 2} == 0 {{ a = a / 3 + j; }} else {{ a = a.wrapping_mul(5).wrapping_add({i % 11}); }}
        j += 1;
    }}
    a.wrapping_add({i})
}}
""")
    cpp.append(f"""static uint64_t f{i}(uint64_t x) {{
    uint64_t a = x * {k} + {i};
    uint64_t j = 0;
    while (j < 4) {{
        if (a % {k + 2} == 0) {{ a = a / 3 + j; }} else {{ a = a * 5 + {i % 11}; }}
        j += 1;
    }}
    return a + {i};
}}
""")
ryn.append("fun main() {\n    mut total: u64 = 0\n" + "".join(f"    total += f{i}({i})\n" for i in range(N)) + "    echo total\n}\n")
rs.append("fn main() {\n    let mut total: u64 = 0;\n" + "".join(f"    total = total.wrapping_add(f{i}({i}));\n" for i in range(N)) + "    println!(\"{}\", total);\n}\n")
cpp.append("int main() {\n    uint64_t total = 0;\n" + "".join(f"    total += f{i}({i});\n" for i in range(N)) + "    std::printf(\"%llu\\n\", (unsigned long long)total);\n}\n")
open("big.ryn", "w").write("".join(ryn))
open("big.rs", "w").write("".join(rs))
open("big.cpp", "w").write("\n".join(cpp))
