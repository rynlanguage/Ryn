# Ryn

**Reliable. Fast. Native.**

Ryn is an early native programming language and compiler implemented in Rust. Version `0.0.5` expands the standard library with interactive IO, collections, filesystem, time, math, process, system, thread, and networking APIs, and adds realtime keyboard input on Windows and Linux. The Rust compiler checks the language and emits native machine code with Cranelift.

## Why Ryn exists

Ryn explores a language with readable source, strong static types, predictable costs, and native performance. The long-term aim is to make ordinary code simple to write while the compiler handles the hard analysis. Ryn is not intended to be Rust with different syntax, and its current bootstrap does not define the eventual memory model or complete language design.

## Status

The `0.0.5` milestone expands the callable standard library and runtime while retaining the experimental self-hosted frontend. Ryn remains an experimental compiler for small programs and compiler experiments, not yet a general-purpose production language.

Implemented syntax and behavior include:

- `//` line comments and nestable `/* ... */` block comments.
- Functions, typed parameters and results, function calls and recursion, with early `return` statements or tail-expression returns. `main` may be `fun main()` or `fun main() -> i32`; the latter supplies the native process exit code.
- Generic functions, structures, enums, and source-order type aliases specialize concrete type arguments; function calls accept inferred or explicit arguments, such as `convert::<i32>(value)`. Generic structures support direct type-parameter fields, collection fields such as `Vec<T>`, and nested generic structures when the inner template is declared first. Generic enums support direct type-parameter payloads, nested generic enum payloads, and `choose`. Generic aliases can be imported through project modules. Recursive generic fields and trait bounds are not implemented yet. See [generic functions](examples/generic_functions.ryn), [explicit type arguments](examples/generic_explicit.ryn), [generic structures](examples/generic_structs.ryn), and [generic enums and aliases](examples/generic_enums.ryn).
- Inferred declarations use `name := value`; explicitly typed declarations use `name: Type = value`. Prefix either form with `mut` to allow mutation, for example `mut count := 0` or `mut count: i32 = 0`.
- Numeric arithmetic and comparisons, explicit numeric casts with `as`, integer `%`, integer bitwise `~`, `&`, `^`, `|`, `<<`, and `>>`, equality for matching numeric, boolean, string, and structure values (structures compare fields recursively), `!`, and short-circuiting `&&` and `||`.
- `when`/`else when`/`else` statements and value-producing `when` expressions with one value per branch, `while`, exclusive integer-range `for` loops, and consuming `for item in Vec<T>` loops with `break` and `continue`, `echo`, simple string interpolation, numeric compound assignment (`+=`, `-=`, `*=`, `/=`, `%=`), and integer bitwise compound assignment (`&=`, `|=`, `^=`, `<<=`, `>>=`) on mutable locals and structure fields.
- String literals support `\0`, `\n`, `\r`, `\t`, `\"`, `\\`, and Unicode scalar escapes in the form `\u{1F980}`. Invalid, surrogate, and out-of-range Unicode values are rejected. Strings carry an explicit length, so an escaped NUL is preserved by `echo`.
- Owning UTF-8 `String` values constructed with `String()` or `String(text)`, with methods for append/push, clone, concat, byte/scalar indexing, copying slices, trim, text predicates, `find`, and `split`. `find` returns `Option<u64>` containing the zero-based Unicode scalar index, or `Option::None`; `split` returns an owning `Vec<String>`. `char` literals and Unicode scalar ordering are supported. Ryn Guard checks moves across assignments, function calls, structures, branches and loops; String owners and temporaries are freed on normal scope exits, returns and loop exits. Restricted custom destructors support move-only resource-handle structs declared with `#[drop(function)]`; their current field/layout limits are documented in the [ownership model](docs/memory-model.md). See also the runnable [dynamic String example](examples/dynamic_strings.ryn).
- `String(number)` formats any signed integer, unsigned integer, `f32`, or `f64`. `value.to_i8()` through `to_i64()`, `to_u8()` through `to_u64()`, and `to_f32()`/`to_f64()` parse a String into the named type. The `try_to_*` forms return `Option<T>` for malformed or out-of-range text; see the [numeric String examples](examples/string_numbers.ryn) and [fallible parsing example](examples/string_parse_options.ryn).
- Structures with named fields, including nested by-value structures, named-field construction, field reads and mutation through `mut`, and pass/return by value. `echo` displays structures in declaration order as `Type { field: value }`, including nested values; strings inside structures are printed without quotes, like standalone strings. Cyclic by-value layouts are not supported yet.
- Type aliases use `type Name = ExistingType` or `type Name<T> = ExistingType<T>` and can name primitive, structure, enum, vector, or array types. Aliases must appear before uses in the source file; they do not create distinct nominal types. See the [type aliases example](examples/type_aliases.ryn) and [generic enum alias example](examples/generic_enums.ryn).
- Enums with unit and tuple-style variants, constructors such as `Data::Text(String("Ryn"))`, exhaustive `choose` expressions, and payload bindings. Enum payloads can own `String` and `Vec<T>` values; pattern matching transfers bound values and generated cleanup releases remaining payloads. Enums print directly and inside interpolation as `Variant` or `Variant(value, ...)`; nested enum payloads and generic enum declarations are not implemented yet. See [the enum example](examples/enums.ryn) and [the enum formatting example](examples/enum_formatting.ryn).
- Fixed arrays such as `[i32; 4]` support literals, by-value parameters and returns, bounds-checked reads, and indexed assignment through mutable locals. Elements can be scalar values, `str`, owning `String`, `Vec<T>`, `Map<K,V>`, enums, structures, or nested arrays; reading an owning element clones it, while assignment replaces and drops the old value. Clone/drop-supported arrays can be passed as call-scoped `&[T]` views. See [the arrays example](examples/arrays.ryn), [the structure array example](examples/struct_arrays.ryn), and [the enum array example](examples/enum_arrays.ryn).
- `Vec<T>` supports scalar and owning String elements, plus `Map<K,V>` elements with deep clone/drop callbacks. It provides `push`, `pop() -> Option<T>`, `get(index) -> Option<T>`, `first() -> Option<T>`, `last() -> Option<T>`, `contains`, capacity management, indexed reads/writes, insert/remove/take, clear, clone, and reverse. `pop()` transfers the last element into an owning `Some` and returns `None` for an empty Vec; `get`/`first`/`last` clone supported elements into the returned Option. Sorting supports numeric and char values with type-aware ordering, and String values by UTF-8 content while moving owning handles without cloning or dropping them. `contains` supports scalar and String equality. `Set<T>` is not yet supported as a Vec element. Restricted custom-destructor resource handles, including through nested ordinary structs, are supported as move-only Vec elements with drop callbacks. They support move-in, replacement, take, clear and consuming iteration; cloning, indexed reads and slices are rejected. Read-only `&[T]` parameters can borrow Vec ranges with `as_slice()` or `slice(start, end)` when the element has generated clone/drop support. Indexing and iteration clone owned elements for the callee; fixed arrays can also be passed as slices, using a call-scoped flattened stack buffer. Storing or returning a slice is rejected. See [the slice example](examples/slices.ryn) and [the array slice example](examples/array_slices.ryn). `Map<K,V>` and its `HashMap<K,V>` spelling support integer, bool, char, and String keys; values may be scalars, String, Vec, nested maps, structs with supported copyable or owned fields (including `get() -> Option<Struct>`), structs with custom destructors (insert/replace/remove/cleanup), or a move-only struct whose ordinary fields nest custom-drop resources. Custom-drop Map values, directly or through ordinary struct wrappers, are destroyed on replacement, removal, clear, and scope exit; `get` and clone are rejected for these move-only values. Such Maps can also be fields of ordinary owning structs. Struct values with supported owned fields use generated clone/drop callbacks for map insertion, replacement, removal, map cloning and cleanup; `.get()` returns an owning `Option<Struct>`, and choose bindings or `?` transfer nested owners safely. Custom-destructor values cannot be cloned or retrieved; custom-drop resources may be stored in regular nested structs and are moved/dropped with those structs, while whole fixed arrays may contain these resources through ordinary structs; nested custom-drop values in enum payloads remain unsupported. `keys()` returns an owning `Vec<K>` of cloned keys and `values()` returns an owning `Vec<V>` of cloned values, so `for key in map.keys()` iterates a Map with the existing Vec loop; `values()` is rejected for move-only value types. See [the map iteration example](examples/map_iteration.ryn). Vec supports move-only elements that contain resources through ordinary struct fields. `get` returns `Option<V>` for cloneable supported values, so a missing key yields `None`; use `choose` or `?` to handle it. `Set<T>` currently uses the same hashed map runtime with boolean marker values and provides `add`/`insert`, `contains`, `remove`, `len`, `is_empty`, `clear`, and clone for supported key types. General user-defined generic types and a distinct Set representation remain unsupported. See [the Map and Set example](examples/maps.ryn).
- File-scoped namespaces qualify declared functions and types: `namespace math::integer;` declares symbols such as `math::integer::add`, and `namespace;` returns to the root namespace. Calls can use full `::` paths, and unqualified function calls inside a namespace resolve in that namespace. See [the namespace example](examples/namespaces.ryn).
- `extern "C"` imports scalar C functions, and `#[repr(C)]` marks structures for C layout. A one-field integer, bool, or char repr(C) record can currently be passed to and returned from an imported function; [the FFI example](examples/ffi_repr_c.ryn) calls native C-ABI shims and verifies both directions and record layout. Float-field and multi-field aggregate ABI are supported for packed records up to 8 bytes; [the float record example](examples/ffi_float_records.ryn) covers those paths. Raw-pointer parameters, dynamic library loading through `load_library`/`load_symbol`, and typed function pointer calls are implemented; [the dynamic library example](examples/dynamic_library.ryn) resolves `abs` from `ucrtbase.dll` and calls it through a typed pointer. Ryn functions coerce to typed function pointers with matching scalar or pointer signatures through `handler as HandlerFn` or an annotated binding, and native code can call those pointers back; [the function pointer example](examples/function_pointers.ryn) invokes coerced functions indirectly and receives a Win32 `EnumWindows` callback that mutates a Ryn local through a raw pointer. Records larger than 8 bytes are not supported yet.
- Process arguments through `arg_count() -> u32` and `arg(index: u32) -> str`. The count excludes the executable path, indexes start at zero, and an out-of-range index returns an empty string; compare with `arg_count()` to distinguish it from a deliberately empty argument. Non-Unicode operating-system arguments are converted lossily to valid UTF-8.
- The embedded standard library exposes the following verified surface. `std::io`: `print`, `println`, `eprint`, `eprintln`, `read_line`, `ask`, `stdin_read`, `stdin_read_line`, `stdout_write`, `stderr_write`, `stdout_flush`, and `stderr_flush`. `stdin_read` consumes input through EOF; `read_line` consumes one line; `ask(prompt)` writes and flushes the prompt before reading one line. `std::keyboard`: `Key` covers A-Z, digits, arrows, Space, Enter, Escape, navigation, modifiers, F1-F12, and common punctuation/numpad keys; `key_down`, `key_pressed`, `key_released`, and `read_key` provide held, transition, and blocking key input. Press/release edges stay latched until queried. Windows polls virtual-key state. Linux reads evdev key transitions from readable `/dev/input/event*` devices; realtime keyboard input requires OS permissions to those devices. `read_key()` reports a runtime diagnostic if no event device can be opened. See [the realtime input example](examples/realtime_input/src/main.ryn). `std::fs`: `read`, `read_text`, `read_file`, `write`, `append`, `create_dir`, `create_dir_all`, `remove_file`, `remove_dir`, `remove_dir_all`, `copy_file`, `move_file`, `rename`, `read_dir`, `exists`, `is_file`, `is_dir`, plus `File::open/create/append/read/read_text/write` and `file.read/read_line/read_text/write/flush/close`. Static file operations and instance methods can share names because `Type::method(...)` and `value.method(...)` are kept as distinct signatures. `File` is a move-only owned handle, dropped once by Ryn Guard; methods that take `mut self` retain the handle until scope cleanup. `std::path`: `new`, `from`, `Path::new`, and methods `exists`, `is_file`, `is_dir`, `is_absolute`, `filename`, `extension`, `parent`, `join`, `absolute`, `canonical`, `to_string`; canonicalization returns an empty String when the OS lookup fails. `std::env`: `get`, `env`, `set`, `set_env`, `remove`, `current_dir`, `set_current_dir`, `home_dir`, `temp_dir`, `executable_path`. `std::time`: `sleep`, `unix`, `instant_now`, `monotonic`, `instant_elapsed`, `Instant::now/elapsed`, `Time::now/unix`; wall-clock timestamps and monotonic elapsed seconds are separate. Duration suffixes `ms`, `s`, `min`, and `h` convert to milliseconds for `sleep`. `std::math`: `abs`, `min`, `max`, `clamp`, `sqrt`, `pow`, `floor`, `ceil`, `round`, `sin`, `cos`, `tan`, `log`, `log2`, `log10`, `random`, `random_range` (all use `f64`). `std::system`: `exit`, `panic`, `assert`, `assert_message`, `os`, `arch`, `cpu_count`, `hostname`. `std::thread`: `Thread::spawn/join/id` and `yield_thread` (from `std::time`); workers must be zero-argument `extern "C" fun()` functions without captured state, and dropping an unjoined Thread joins it. `std::process`: `Process::new/arg/args/env/cwd/spawn/wait/kill`, `run`, `run_capture`, `run_capture_args`; captures return `ProcessResult { exit_code, stdout, stderr }`. Spawn inherits parent standard streams. Captured invalid UTF-8 is decoded lossily; the process builder APIs return status codes/booleans and do not yet expose piped per-process streams. `std::net`: `TcpStream::connect/read/write/close`, `TcpListener::bind/accept/close`, `UdpSocket::bind/connect/read/write/close`, and `Dns::resolve`; reads collect TCP bytes through EOF or receive one UDP datagram (up to 65,535 bytes). Socket content is decoded lossily as UTF-8, and failed connection setup yields inert handles.
- `String` methods available on owning `String` include `len`, `is_empty`, `contains`, `starts_with`, `ends_with`, `trim`, `trim_start`, `trim_end`, `to_lower`, `to_upper`, `replace`, `split`, `lines`, `chars`, `bytes`, `substring`, `repeat`, `reverse`, `clone`, `concat`, `append`, `clear`, `push`, `byte_at`, `char_at`, `char_count`, `slice`, `slice_chars`, `find`, and numeric conversions `to_i8/i16/i32/i64`, `to_u8/u16/u32/u64`, `to_f32/f64`. `String(number)` and `to_string()` format supported numeric values. `parse::<T>()` and `try_parse::<T>()` support these ten numeric types; malformed `parse` exits with a runtime diagnostic and `try_parse` returns `Option<T>`. `split`, `lines`, `chars`, and `bytes` return owning `Vec` values. `substring(start,end)` indexes Unicode scalar values; `slice(start,end)` indexes UTF-8 bytes and requires valid character boundaries.
- `Vec<T>` exposes `push`, `pop`, `get`, `set`, `first`, `last`, `len`, `is_empty`, `clear`, `contains`, `remove`, `insert`, `reverse`, `sort`, and `clone`. `get`, `pop`, `first`, and `last` return `Option<T>`; supported elements include scalars and owned Strings, plus Maps, structs and move-only resources subject to documented clone/drop constraints. Sorting supports numbers, chars, and Strings. `Map<K,V>` exposes `insert`, `get`, `remove`, `contains_key` (`contains` is also accepted), `len`, `is_empty`, `clear`, `keys`, `values`, and `clone`; `get` returns an owning `Option<V>` for cloneable supported payloads. `Set<T>` exposes `insert`/`add`, `remove`, `contains`, `len`, `is_empty`, `clear`, and `clone`, backed internally by Map storage. `Option<T>` has `is_some`, `is_none`, `unwrap`, `unwrap_or`, and `expect`; `Result<T,E>` has `is_ok`, `is_err`, `unwrap`, `unwrap_err`, `unwrap_or`, and `expect`. `Option.map` is available after `use std::option`; `Result.map` and `Result.map_err` are available after `use std::result`. They accept a typed Ryn function pointer callback; callback arguments and results may include owned pointer-backed values such as `String`, `Vec`, `Map`, `Set`, and enums, which Ryn Guard moves into the callback and cleans up with the returned enum. Extraction of pointer-backed, struct, and fixed-array payloads returns an independent owning value, leaving the source enum valid. Struct/array payloads cannot contain custom-drop resources. Capturing closures are not supported. `unwrap_or` evaluates its fallback eagerly. `try_parse` yields Option; `parse::<T>()` is currently a terminating conversion rather than a Result-returning API.
- Remaining limitations include the lack of capturing closures for Option/Result mapping. Realtime keyboard input is implemented on Windows and Linux; Linux evdev press/release state depends on OS device permissions; without access to an event device, state queries return `false` and `read_key()` exits with a diagnostic. macOS and other platforms do not yet have a realtime keyboard backend. `Path::canonical` returns an empty String when canonicalization fails; process `spawn` does not offer captured child streams, although `run_capture` does; `File::open/create` report OS errors by terminating with a runtime diagnostic. The built-in self-hosted frontend only covers a limited type-checking subset; stdlib features are verified through the Rust compiler/CLI path.
- `ryn new` creates a project with `ryn.yaml` and `src/main.ryn`; `ryn check`, native `ryn build`, and `ryn run` validate and use the manifest when given a project directory. `build.optimize` accepts `speed`, `size`, and `none`; local `path` dependencies and their transitive local dependencies resolve modules from each package's `src/` directory. Version and Git dependencies can be declared but are not fetched. Project commands use `src/main.ryn`, and their default executable is written under `build/`. `ryn clean <project-dir>` removes only that project's direct `build/` directory and refuses a symbolic link at that path. Rust library APIs are available through `ryn::check`, `ryn::check_recovering`, `ryn::check_source`, `ryn::check_source_recovering`, `ryn::compile`, `ryn::compile_source`, and the public compiler modules.
- `ryn check` gathers recoverable lexical errors and, when lexing succeeds, recovers between structure fields, function parameters, call arguments, structure literal fields, and statements, inside nested blocks, and across top-level declarations to report independent parser errors together. It also checks control-flow bodies after malformed conditions or range headers. Semantic analysis reports duplicate structure and function declarations together in source order. With unique top-level names, it reports all invalid field declarations, function parameter/result types, and invalid `main` signatures. With valid signatures, it reports all duplicate parameter names and skips affected bodies. Other bodies collect errors across independent statements and nested blocks, including each missing name in an `echo` interpolation, errors in arguments to known or unknown calls (including known calls with the wrong arity), independent field initializer errors in structure literals, and errors from both operands of a binary expression when both can be checked independently. It also checks an assignment's right-hand expression even when its target is invalid, a local initializer when its declaration is invalid, and return expressions when the function has no declared result type. A failed new local declaration stops later statements in that block to avoid cascading unknown-name errors. An invalid `when` or `while` condition does not prevent checking its branches or loop body. Tail-return validation also reports independent errors unless recovery stopped at a failed function-level local declaration. Recovering semantic analysis reports independent recursive by-value structure layout cycles together; other global layout errors stop at the first issue.

Integer literals may be decimal, binary (`0b1010`), or hexadecimal (`0x2a`). Binary and hexadecimal prefixes may use uppercase letters, and `_` may group digits in any integer base. Decimal integers and floating-point literals may also use `_` between digits, including in the fractional part and exponent. Separators at the start or end of a digit sequence, next to a decimal point or exponent marker, or repeated together are rejected. Numeric conversions are explicit: mixed numeric operands are rejected unless converted with an explicit cast. Integer literals use an expected integer type when context provides one, or default to `i64`; floating-point literals use an expected `f32` or `f64`, or default to `f64`. Signed integer unary `-` and integer `+`, `-`, and `*` wrap at the operand width. Signed integer division truncates toward zero; `%` returns the corresponding signed remainder. Unsigned `/` and `%` use unsigned division and remainder. The signed minimum divided by `-1` wraps to that minimum, and its remainder is `0`. Integer `/` or `%` by zero prints a runtime diagnostic to stderr and exits with code `1`. Floating-point arithmetic follows IEEE-754: division by zero can produce infinity or NaN, NaN is unequal to itself, and ordered comparisons with NaN are false. Non-finite floating-point literals are rejected; non-finite runtime results are allowed.
Numeric casts use `value as target`. Integer widening extends the source sign for signed values and fills with zero for unsigned values; integer narrowing keeps the low bits. A cast to a same-width integer type with different signedness preserves the bits and changes their interpretation. Integer-to-float conversions round to nearest, ties to even. `f32` to `f64` is exact; `f64` to `f32` rounds to nearest, ties to even. Float-to-integer conversions truncate toward zero and saturate to the target type's range; NaN converts to zero.

Bitwise `~`, `&`, `^`, and `|` require integer values; binary operands must have the same integer type. Complement and binary operations keep that type's width and signedness. Shifts take an integer value on the left and a `u32` count on the right, and keep the left operand's type. A shift count at least as large as the left type's width produces zero for `<<` and unsigned `>>`; signed `>>` fills with the sign bit. Arithmetic binds more tightly than shifts, shifts bind more tightly than bitwise `&`, then `^`, then `|`; bitwise operators bind more tightly than comparisons, which bind more tightly than logical `&&` and `||`.

During semantic recovery, an invalid `for` start bound does not hide errors in the end bound. If the end bound has an integer type, the loop body is checked using that type as well. A duplicate loop-variable name also does not hide independent range-bound or loop-body errors.

Function arguments, binary operands, and structure field initializers evaluate from left to right in source order. Structure values are then stored, passed, and printed in field declaration order.

## Object model

Behavior attaches to types with `extend`; data stays in `struct`. Methods take `self` for a shared borrow or `mut self` for a mutable borrow, so Ryn Guard knows whether a method reads or modifies the receiver, and mutations through `mut self` propagate to the caller's local. Associated functions (no `self`) and constants are called through the type name:

```ryn
struct Player {
    pub name: String,
    health: i32,
}

extend Player {
    pub fun new(name: str) -> Player {
        return Player { name: String(name), health: 100 }
    }

    pub fun health(self) -> i32 => self.health

    pub fun damage(mut self, amount: i32) {
        self.health -= amount
    }

    pub const MAX_HEALTH: i32 = 100
}
```

Fields are private to their module unless marked `pub`; methods are private unless marked `pub`. Void `mut self` methods chain in place on the receiver (`player.damage(10).heal(5)`), and only the last link of a chain may return a value. See [the objects example](examples/objects.ryn).

`shape` declares a structural contract: any type whose methods satisfy the required signatures conforms automatically — no implementation declaration needed. Methods with bodies provide default implementations that are materialized per conforming type. Shapes constrain generics (`fun tick<T: Entity>(entity: T)`, combined bounds with `+`), checked at the call site during monomorphization (`R0450`), and `extend Player as Entity` validates an explicit conformance:

```ryn
shape Entity {
    fun update(mut self, delta: f32)

    fun enabled(self) -> bool => true
}

fun tick<T: Entity>(entity: T) -> T {
    entity.update(0.016)
    entity
}
```

See [the shapes example](examples/shapes.ryn). Built-in types extend the same way — `extend String { fun empty(self) -> bool { ... } }` ([the String extension example](examples/string_extend.ryn)).

`#[derive(Clone, Eq, Hash)]` generates capabilities for scalar-friendly structs: `Clone` synthesizes a deep `Type::clone()` method, `Hash` (with `Eq`) allows the struct as a `Map` key, and `Eq` documents recursive field equality. String interpolation reads field paths — `echo "HP: {player.health}"` — with per-segment diagnostics.

Arithmetic operators resolve through `extend` methods: defining `fun add(self, rhs: Vec3) -> Vec3` makes `a + b` work (`sub`, `mul`, `div`, `rem` map the same way), with the right operand's literal type taken from the method signature ([the operators example](examples/operators.ryn)). Associated types, `shape`-spelled operator contracts, and conditional extensions remain future work.

## Self-hosting (0.0.4)

`examples/self_hosted_project` is a Ryn-written frontend for Ryn: a source manager (UTF-8 line indexing, LF/CRLF/CR handling), a lexer (keywords, identifiers, decimal/binary/hex integers, floats, strings with escapes, characters, punctuation, operators), and a parser (functions with parameters and return types, declarations and assignments, `when`/`else`, `while`, `for`, `break`/`continue`/`return`, `echo`, expression statements, calls, field/method access, indexing, struct literals). The `main.ryn` CLI reads a file and reports the first lexical or parse diagnostic with line/column, or `ok`:

```ryn
ryn run examples/self_hosted_project -- examples/hello.ryn
```

Pass `--check` before the source path to run the experimental type checker:

```ryn
ryn run examples/self_hosted_project -- --check examples/hello.ryn
```

It currently checks primitive value classes (integer, float, boolean, string, and character), `Vec<T>` parameter and return signatures, local declarations and assignments (including immutable-local and value-class mismatches), simple function signatures and calls, and boolean control-flow conditions. Numeric widths and signedness are currently grouped together. It reports the first type error. Container operations, nested generic types, method typing, full borrow checking, and full Rust-compiler parity are not implemented yet.

Differential tests (`tests/self_hosted_frontend.rs`) check the parser against samples the Rust compiler accepts and rejects. Type-checking tests cover the supported subset and verify that unsupported or invalid programs return diagnostics instead of crashing. This is the first stage of the self-hosting path; Ryn Guard and native code generation remain future stages.

## Ryn example

The runnable [quick start](examples/quick_start.ryn) demonstrates a function, inferred and mutable locals, compound assignment, a value-producing conditional, and string interpolation:

```ryn
fun add(a: i32, b: i32) -> i32 {
    a + b
}

fun main() {
    name := "Ryn"
    mut health := 100

    health -= 20

    status := when health > 0 { "alive" } else { "down" }
    echo status

    when health > 0 {
        echo "Hello, {name}!"
        echo add(20, 22)
        echo health
    }
}
```

The [conditional-expression example](examples/if_expression.ryn) covers typed branch values, nested `else when`, and selected-branch evaluation through the native backend.

The [integer semantics example](examples/integer_semantics.ryn) demonstrates fixed-width wrapping, signed division toward zero, minimum-value division edge cases, and unsigned division and remainder at each integer width.

The [numeric casts example](examples/integer_casts.ryn) demonstrates integer sign extension, zero extension, narrowing, signedness reinterpretation, float conversion and saturation, and converting a computed shift count.

The [bitwise example](examples/bitwise.ryn) demonstrates integer masks, width-preserving complement, shifts, compound flag updates, and operator precedence.

The [range-loop example](examples/for_ranges.ryn) demonstrates exclusive integer ranges, one-time ordered bound evaluation, `break`, `continue`, and an unsigned range near its type maximum.

The [string-escapes example](examples/string_escapes.ryn) demonstrates escaped NUL, quote, backslash, and Unicode characters in native string output.

Write a range loop as `for index in start..end { ... }`. The bounds must have the same integer type. The start is included and the end is excluded; both expressions are evaluated once from left to right before iteration. The immutable `index` binding is available only inside the loop body, and the loop advances it by one.

Write `for item in values { ... }` to consume a `Vec<T>` in order. Each element moves into an immutable loop binding; the vector and any unconsumed elements are cleaned up on loop exit. See the [Vec iteration example](examples/vec_iteration.ryn).

The [structures example](examples/structs.ryn) shows nested field initialization, reads and updates, pass/return by value, and whole-structure output both directly and inside string interpolation.

The [exit-code example](examples/exit_code.ryn) shows a program returning a nonzero native process status.

The [process-arguments example](examples/arguments.ryn) reads the number of command-line arguments and the first argument.

The [filesystem example](examples/filesystem.ryn) demonstrates creating directories, writing and reading UTF-8 files, checking file and directory paths, and deleting files and empty directories. `create_dir(path)` creates exactly one directory and fails if its parent is missing; `create_dir_all(path)` recursively creates parents. `try_read_file` returns `Option<String>` for success/failure; `read_file_result` returns `Result<String, i32>` with distinct codes for missing files, permissions, invalid UTF-8, invalid paths, and other I/O failures. The [Result file example](examples/read_file_result.ryn) shows both success and error handling. `read_file` remains the terminating convenience form.

The [system example](examples/system.ryn) demonstrates environment access and direct stdout/stderr writes. `stdin_read()` reads all UTF-8 input until EOF, as shown in the [stdin example](examples/stdin.ryn). `stdin_read_line()` and its public alias `read_line()` read one line and remove its LF or CRLF ending; a blank line returns an empty String, and EOF safely returns an empty String. `ask(prompt)` writes the prompt without a newline, flushes stdout, then reads one line. These are ordinary callable builtins in the existing system API registry, so the compiler can expose them globally without parser changes or a new syntax token; the current frontend does not automatically load stdlib modules as a global prelude. See the [interactive input example](examples/input_test/src/main.ryn). `stdout_write()` writes without adding a newline, and `stdout_flush()` explicitly flushes stdout. `run_process(program)` launches one process without arguments; `run_process_args(program, Vec<String>)` passes UTF-8 arguments. Both return the exit code, or `-1` if launch fails or the process has no numeric exit code. [A Windows process example](examples/process_args_windows.ryn) checks argument delivery and exit status.

Small `#[repr(C)]` aggregates of up to 8 bytes can use integer, boolean, character, `f32`, and `f64` fields with natural C layout on the currently supported Windows x86_64 ABI. [The float-record example](examples/ffi_float_records.ryn) passes and returns float records through runtime-shim functions using the real C ABI.

`exit(code: i32)` terminates the current process with the requested status after releasing live compiler-known owners. Returning an `i32` from `main` is also supported and remains the ordinary way to choose a final status.

`panic(message)`, `assert(condition)`, and `assert_message(condition, message)` write failures to stderr, release live compiler-known owners, and terminate with exit status 1. Generated cleanup includes supported custom destructors, but Ryn does not unwind its frames.

The [panic cleanup](examples/panic_cleanup.ryn) and [assertion cleanup](examples/assert_cleanup.ryn) examples each keep an owning String live on the failure path.

## Build and run

From the repository root:

```powershell
cargo build
cargo run -- --help
cargo run -- --version
cargo run -- check examples/quick_start.ryn
cargo run -- build examples/quick_start.ryn
cargo run -- run examples/quick_start.ryn
cargo run -- run examples/arguments.ryn -- hello "two words"
cargo run -- build examples/quick_start.ryn --output target/quick-start.exe
```

Create and run a project from its directory:

```powershell
cargo run -- new hello-ryn
Set-Location hello-ryn
cargo run --manifest-path ..\Cargo.toml -- check .
cargo run --manifest-path ..\Cargo.toml -- build .
cargo run --manifest-path ..\Cargo.toml -- run .
```

When using an installed `ryn` executable, the commands from inside the project are simply `ryn check .`, `ryn build .`, and `ryn run .`. The project entry point is `src/main.ryn`; default native output goes to `build/debug/<project-name>` (with `.exe` on Windows), while `--release` selects `build/release/`. `ryn new` refuses to overwrite an existing path and creates `ryn.yaml` plus `build/cache`, `build/debug`, and `build/release` directories. Project builds reuse an unchanged, fingerprint-verified executable from `build/cache`; changed inputs trigger a rebuild.

The generated manifest is intentionally small:

```yaml
name: hello-ryn
version: 0.1.0
owner: guest

dependencies:

build:
  optimize: speed
  debug: none
  release: speed
```

`build.optimize`, `build.debug`, and `build.release` accept `speed`, `size`, or `none`. The profile-specific setting overrides `build.optimize`; when omitted, the legacy `build.optimize` value is used. `--release` selects the release output directory and release optimization, while the default build uses the debug profile. Standalone source builds use `none` for debug and `speed` for release. Direct and transitive local path dependencies are resolved as project packages. Git dependencies may specify an optional branch; `ryn lock <project>` writes a `ryn.lock` with the selected commit, and project check/build/run restore that locked revision in the project's ignored `.ryn/git` cache. Registry version dependencies are parsed but not resolved yet.

## Install the latest release

The installers download the latest stable GitHub release, verify its SHA-256 checksum, and install the CLI for the current user. Windows uses `install.ps1` and installs to `%LOCALAPPDATA%\Programs\Ryn`; Linux x86_64 uses `install.sh` and installs to `~/.local/bin`. The Bash installer adds that default directory to Bash startup files when needed.

Run the script from a checkout of this repository:

```powershell
powershell -ExecutionPolicy Bypass -File .\install.ps1
```

```bash
bash ./install.sh
```

The repository is currently private. Sign in with `gh auth login` before running an installer, or provide a short-lived `GH_TOKEN`/`GITHUB_TOKEN` with read access to the repository. The scripts also work without a token if the repository is made public. Linux currently publishes x86_64 binaries; other Linux architectures need a matching release asset before they can be installed.

Remove the project's default build artifacts with `ryn clean .`. This command leaves source files and executables written outside `build/` untouched.

`build` and `run` accept `--release` to select the release output directory and `-o` or `--output` to choose the native executable path. `run` keeps the generated executable at that path after it finishes.

When `main` returns `i32`, `ryn run` exits with that status after flushing the program's standard output. A `main` without a result returns process status `0`.

Pass program arguments to `ryn run` after `--`, for example `ryn run app.ryn -- input.txt "two words"`. The separator is required so the CLI can distinguish its own options from arguments for the Ryn program. `arg_count()` excludes the executable name, and `arg(0)` reads the first supplied value.

If linking fails, an existing executable at the output path is left intact. A new executable replaces it only after linking succeeds.

Ryn function code is generated by Cranelift; the compiler does not transpile Ryn into another high-level language. The current native linker driver is `rustc`, with a small host shim for output and string equality.

Native code generation follows Cranelift's host backend support. The build-and-run path is verified on x86_64 Windows; supported Linux hosts include x86_64, aarch64, s390x, and riscv64. `ryn check` works on other Rust targets too, but `ryn build` and `ryn run` report that Cranelift's native backend is unavailable when it has no backend for that host; 32-bit x86 is one such target.

## Compiler architecture

The `ryn` Rust library exposes source diagnostics, lexer, AST, parser, semantic analysis, typed IR, and native code generation. `ryn::check` and `ryn::compile` accept source text; `ryn::check_source` and `ryn::compile_source` accept a `SourceFile` so callers can retain its path, render diagnostics against the original source, and prevent output from overwriting that file. `CompileError::render` produces source excerpts for source errors and plain messages for build errors. `SourceFile::load` reads a file while preserving its path. Both the CLI and library use the same compiler path.

The native backend emits a host object with Cranelift, then links it with the small runtime shim. The compiler and CLI remain in this repository until another ecosystem component has a concrete reason to be independently versioned.

## Tests

Run `cargo test --workspace` for lexer, parser, semantic, diagnostics, malformed-source no-panic, compile-pass, compile-fail, library API, and native end-to-end coverage. Run `cargo test --release --workspace -- --test-threads=4` as well to exercise the full suite against optimized compiler code. Every file in `examples/` and `tests/programs/pass/` is checked with `ryn check`; files in `tests/programs/fail/` must fail with their expected diagnostic codes.

## Benchmarks

Run `cargo bench --bench compiler_pipeline` to measure lexer and parser throughput on existing examples, ordinary and recovering checks on valid source, recovering checks on malformed source, rendering a batch of independent diagnostics, Cranelift object emission, linking with the host runtime, and full native-build time. Each result reports the median and range from five samples. These local measurements are useful for comparing changes on the same machine; they are not performance guarantees.

## Roadmap

- Continue improving diagnostics, source handling, and the reusable compiler API.
- Expand the language deliberately, with tests for each addition and design review before decisions that shape memory safety, errors, generics, or concurrency.
- Consider a standard library, language specification, editor tooling, and Ryn Pods after the compiler foundations justify those separate efforts.

Standard library and Pods work are planned; they are not part of the current compiler.

## Contributing

Contributions should preserve the native Cranelift pipeline and avoid introducing language semantics by accident. Include tests and update examples or documentation when behavior changes. Before submitting changes, run:

```powershell
cargo fmt --all -- --check
cargo check --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo test --release --workspace -- --test-threads=4
```

## License

Ryn is distributed under the [Mozilla Public License 2.0](LICENSE).
