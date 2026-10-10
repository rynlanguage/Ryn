# Ryn

<p align="center">
  <img src="assets/Banner.png" alt="Ryn: Reliable. Fast. Native." width="800">
</p>

**Reliable. Fast. Native.**

Ryn is a statically typed systems language that compiles to native machine code. There is no garbage
collector: ownership is checked at compile time by Ryn Guard and released deterministically. The
compiler's lexer, parser, lowering and native code generator are written in Ryn, and the native
compiler rebuilds itself from its own sources.

## Status

Version 0.1.6 is under development on `release/v0.1.6-native-first`; the latest published release is
[0.1.5](https://github.com/rynlanguage/Ryn/releases/tag/v0.1.5).

- On Linux x86-64, `ryn check`, `ryn build` and `ryn run` use the self-hosted native compiler
  (`rync`) by default — no rustc, C compiler, linker or C library is invoked. `--backend cranelift`
  selects the Rust/Cranelift backend, which also serves other targets.
- The native compiler reproduces itself: `ryn bootstrap --native` builds stage 2 and stage 3 and
  requires them to be byte-identical.
- The Rust analysis still runs next to the self-hosted analysis on the default path, for the R0904
  consistency check and for diagnostics the self-hosted analysis does not yet produce.
- Ryn is experimental. It is not a general-purpose production language yet.

## Example

```ryn
struct Point {
    x: i32,
    y: i32,
}

fun quadrant(point: Point) -> str {
    when point.x >= 0 && point.y >= 0 {
        "first"
    } else when point.x < 0 && point.y >= 0 {
        "second"
    } else when point.y < 0 && point.x < 0 {
        "third"
    } else {
        "fourth"
    }
}

fun main() {
    origin := Point { x: 0, y: 12 }
    name := "Ryn"
    echo "Hello, {name}!"
    echo quadrant(origin)
    echo quadrant(Point { x: -3, y: 4 })
}
```

```console
$ ryn run examples/hello.ryn
Hello, Ryn!
```

The `quadrant` function above returns `"first"` for `Point { x: 0, y: 12 }`.

More runnable programs are in [`examples/`](examples/): ownership and destructors, generics,
patterns, collections, file and process APIs, FFI, and the self-hosted compiler driver.

## Install

Download a release archive from [GitHub Releases](https://github.com/rynlanguage/Ryn/releases)
(Linux x86-64 and Windows x86-64, with SHA-256 checksums), or build from source:

```sh
cargo build --release
./target/release/ryn --version
```

The release installers verify the archive checksum and install for the current user:

```powershell
powershell -ExecutionPolicy Bypass -File .\install.ps1
```

```bash
bash ./install.sh
```

The Linux `rync-<version>-linux-x86_64.tar.gz` archive contains the self-hosted compiler, its Ryn
sources, the standard library and the native bootstrap script.

## Quick start

```sh
ryn new hello-ryn          # scaffold a project (ryn.yaml + src/main.ryn)
cd hello-ryn
ryn check .                # analyse
ryn build .                # compile to build/debug/<name>
ryn run .                  # compile and run
ryn run app.ryn -- arg     # pass arguments after --
ryn clean .                # remove build artifacts
```

A single file works too: `ryn check main.ryn`, `ryn build main.ryn -o main`, `ryn run main.ryn`.
`main` returning `i32` becomes the process exit code.

Useful options: `--backend native|cranelift`, `--frontend ryn|rust` (Cranelift pipeline),
`--release`, `-o/--output <path>`.

## Language

- Functions with typed parameters and results, early `return` or tail-expression returns, `mut`
  bindings, `name := value` and `name: Type = value` declarations.
- Control flow: `when`/`else when`/`else` as statements and as values, `while`, `for i in start..end`
  and `start..=end`, `for item in vec`, `break`, `continue`, `defer { ... }`.
- `struct` data with `extend` methods (`self`, `mut self`, associated functions and constants),
  `shape` contracts with generic bounds, `#[derive(Clone, Eq, Hash)]`, operator overloading through
  `add`/`sub`/`mul`/`div`/`rem`, and `#[drop(f)] destructors for move-only resource handles.
- `enum` with unit and tuple payloads, exhaustive `choose` with literal, tuple, struct, nested and
  binding patterns.
- Ownership: owning `String`, `Vec<T>`, `Map<K,V>`, `Set<T>`, `Option<T>`, `Result<T,E>` and
  payloads; moves, borrows and deterministic cleanup checked by Ryn Guard; no GC, no refcount.
- Generics over functions, structs, enums, aliases and `extend<T>` blocks, with inference and
  explicit type arguments.
- `const` items with a compile-time evaluator (`comptime(...)`, `static_assert(...)`,
  `field_count::<T>()`, `variant_count::<T>()`, `type_name::<T>()`, `has_field::<T>("name")`,
  `#[derive(Default)]`).
- Fixed-width integers with wrapping arithmetic, explicit `as` casts, IEEE-754 `f32`/`f64`, `char`
  and UTF-8 strings with `\u{...}` escapes.
- `extern "C"` imports, `#[repr(C)]` records, raw pointers, function pointers and callbacks.
- `?` propagation, `??` null-coalescing, `?.` optional field access, and pipes `value |> f`.

The full language surface, including known limits, is documented in
[docs/language-0.2.md](docs/language-0.2.md); ownership rules are in
[docs/memory-model.md](docs/memory-model.md).

## Standard library

`std::io`, `std::fs`, `std::path`, `std::env`, `std::math`, `std::option`, `std::result`,
`std::process`, `std::thread`, `std::time`, `std::system`, `std::net`, `std::keyboard` and
`std::arena`. The sources live in [`stdlib/std/src`](stdlib/std/src) and compile with the native
compiler.

## Documentation

| Document | Contents |
| --- | --- |
| [docs/language-0.2.md](docs/language-0.2.md) | Language features, test distribution, known limits |
| [docs/memory-model.md](docs/memory-model.md) | Ownership, moves, borrows, Ryn Guard |
| [docs/native-compiler.md](docs/native-compiler.md) | `rync`, its bootstrap and compatibility |
| [docs/self-hosting-plan.md](docs/self-hosting-plan.md) | Plan and progress toward full self-hosting |
| [docs/native-ownership-validation.md](docs/native-ownership-validation.md) | Ownership and bootstrap measurements |
| [docs/native-float-validation.md](docs/native-float-validation.md) | Freestanding float runtime validation |
| [CHANGELOG.md](CHANGELOG.md) | Release history |

## Testing

```sh
cargo test --workspace                              # unit, integration and end-to-end tests
cargo test --release --test suite                   # the 628-program language suite
RUST_MIN_STACK=16777216 cargo test                  # deep compiler tests need a larger thread stack
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). For questions and bug reports see
[SUPPORT.md](SUPPORT.md); security issues are handled as described in
[SECURITY.md](SECURITY.md). Contributors are listed in [CONTRIBUTORS.md](CONTRIBUTORS.md).

## License

Ryn is distributed under the [Mozilla Public License 2.0](LICENSE).
