# Ryn, Rust and C++ on the same programs

The same algorithm in three languages: `fib` (recursion), `collatz` (a hard-to-predict branch), `sieve` (a vector),
`hashmap` (2 000 000 keys), `fsum` (`std::simd::sum_f32` against a plain `f32` loop), `list` (5 000 000 nodes from
`std::alloc::Pool` against `Box` and `new`), `hello` (start and size), and `big` (3000 functions, from `gen_big.py`).

```sh
python3 benches/versus/gen_big.py            # writes big.ryn, big.rs, big.cpp in this directory
python3 benches/versus/measure.py "$PWD/bootstrap/rync-seed" fib sieve collatz hashmap fsum list hello big
```

`measure.py` builds with `rync build`, `rustc -O` and `g++ -O2`, runs each program 5 times and keeps the median; the results,
with the machine they were taken on, are in `docs/native-performance.md`. Ryn is slower on the compute-bound programs and
faster on start, size, the build of a large program, the pool allocator and the SIMD loop; the table in the document says by how much.
