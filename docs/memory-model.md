# Owning values and Ryn Guard

The bootstrap compiler now supports an owning, heap-allocated UTF-8 `String`. `str` remains an immutable pointer/length view, including string literals and process arguments. A `String` owns its buffer and has no garbage collector or reference counter.

```ryn
fun decorate(value: String) -> String {
    mut result := value
    result.append("!")
    result
}

fun main() {
    original := String("Ryn")
    result := decorate(original.clone())
    echo original
    echo result
}
```

## Transfers and observations

- Initializing or assigning an owning value, passing it to a by-value function parameter, and returning it transfer ownership. `String` fields make their containing structure owning too. Primitive values, `str`, and structures containing only those values retain their existing copy semantics.
- `echo`, equality, and String methods borrow their operands for the duration of the operation. `.clone()` explicitly allocates a separate owner. A mutable method requires a `mut` root binding, including for a field such as `record.text.append(...)`.
- Moving a field makes that field unavailable. Other fields remain usable, and a mutable assignment can reinitialize a moved field. Reading or moving the whole structure requires all its owning fields to remain initialized.
- Ryn Guard rejects use after move, moves while an earlier operand still borrows the value, and loop backedges that would reuse an owner without reinitializing it. Branches merge their availability states, and returning branches do not affect the state of continuing branches.
- An owning value produced by a `when` expression transfers the selected branch's value. Observing that resulting temporary destroys it after the observation.

The initial Ryn Guard pass tracks owning values and temporary expression borrows. Experimental scalar references (`&T`, `&mut T`) and raw pointers (`*T`, `&raw mut value`) are available for local scalar variables, including pointer parameters and dereference reads/writes. Direct local-reference returns and unknown call-result forwarding are rejected. Ryn Guard tracks direct local-reference aliases through local initialization, assignment, and control-flow joins; it rejects overlapping shared/mutable aliases to the same scalar in a single call. It tracks return provenance when a function directly returns one reference parameter or a scalar element of a borrowed slice. For Vec/array slice sources, the caller keeps the collection borrowed for the lexical scope of the returned reference binding; moves, Vec mutation/drop, and array element writes are rejected during that scope. Unknown reference results cannot be stored. Lifetimes are lexical rather than non-lexical. Raw dereference is unchecked, and owned/aggregate reference pointees are not supported. User-defined destructors are available for restricted resource-handle structs as described below; generic ownership constraints and the complete Ryn Guard rules remain incomplete.

Fixed arrays use stack/local value slots and support scalars, `str`, `String`, `Vec<T>`, `Map<K,V>`, enums, nested arrays, and supported nested structures. Parameters, returns, and array assignment transfer value ownership. Reading an owning element clones it, so the array retains its owner. Indexed writes through a mutable named array local destroy the replaced owning element. Reads and writes check negative and out-of-range indices at runtime. See the feature audit for current recursive layout limits.

## Borrowed Vec slices

`&[T]` is currently available as a function parameter for elements with generated clone/drop support, including scalars, String, maps, and supported structures, when the caller passes `values.as_slice()` or `values.slice(start, end)` from a `Vec<T>`. A Vec slice is a pointer/length descriptor into the Vec's contiguous element storage. It supports bounds-checked indexing, `len()`, and non-consuming `for` iteration. The source Vec is borrowed for the duration of the receiving call; Ryn Guard rejects moving that Vec into a later argument while the slice argument is still live. Fixed arrays and context-typed array literals can also be passed to `&[T]`; today the compiler copies their flattened SSA values into a call-scoped stack buffer before invoking the function. This provides immutable slice behavior but is not a zero-copy borrow of the array's storage. Slice descriptors cannot be returned, stored in locals or structure fields, or mutated through the slice. `first<T>(items: &[T]) -> &T` can return a reference to an in-bounds scalar element; Ryn Guard tracks a Vec/array source through the call and prevents moves or mutations until the local reference binding leaves scope. References to non-scalar elements, unknown reference results, and general lifetime annotations remain unsupported.

## Type layout queries

`sizeof(Type)` and `alignof(Type)` are compile-time expressions that return `u64`. The current logical target layout assumes 8-byte pointers: integer and boolean widths follow their declared bit widths, `char` uses a 4-byte scalar, `str` and `&[T]` are pointer/length pairs, and owning String/Vec/Map/enum handles occupy one pointer. Arrays use aligned element strides; structures insert field padding and round their final size to the greatest field alignment. `examples/layout.ryn` demonstrates scalar, array, structure, string-view, and owning-handle queries. These operators describe the current Ryn target data layout; they do not make a C ABI promise or add `repr(C)`.

## Destruction

String buffers and custom resource handles are cleaned up exactly once at normal scope exit, `return`, `break`, `continue`, `panic`, failed assertions, and `exit`. Replacing an owning local first evaluates its replacement, then destroys the old value. Moves clear the previous owner's slot so cleanup cannot destroy the transferred value. Temporary owners are destroyed after an observation or a discarded call result.

Scope cleanup visits locals in reverse declaration order; nested built-in owning structure fields are also destroyed in reverse declaration order. Function parameters are destroyed after the function's local owners. A custom destructor is declared with `#[drop(function_name)]` immediately before a non-generic, non-`repr(C)` struct. Its private, non-generic function must accept that struct by value and return no value. The struct's first field must be a raw pointer, and every field must be a scalar or raw pointer. Guard treats the whole struct as a move-only owner, invokes the destructor only while its handle is live, and rejects a direct call to the destructor function. The handle field cannot be overwritten alone; replacing the whole value runs the old destructor first. Reading fields does not move the owner. A custom-destructor struct may be passed or returned by value, including in a fixed array of wrapper structs that is moved as a whole. Array elements containing custom-drop resources remain non-indexable. Reading or replacing a custom-destructor array element by index is rejected because it would require element-level move semantics. `Vec<Resource>` has generated drop glue and supports `push`, `set`, `take`, `clear`, and consuming iteration. Clone, indexed/extracted reads, and slices are rejected for move-only elements. `Map<K, Resource>` and `Map<K, Wrapper>` have generated drop glue when Wrapper contains custom-drop resources through ordinary struct fields. They support move insertion, replacement, `remove`, `clear`, function transfer, and scope cleanup; `.get()` and cloning are rejected for these move-only values. A regular owning struct may hold such a Map as a field, and its cleanup releases the Map and its nested resources. Plain scalar-only struct Map values support insertion, replacement, removal, cleanup, and `.get() -> Option<Struct>` with multi-slot payloads; pattern matching and `?` preserve their fields. Regular structs with supported owned fields use generated clone/drop callbacks for Map insertion, replacement, removal, Map cloning and cleanup. `.get() -> Option<Struct>` returns an owning struct; choose bindings and `?` transfer nested owners safely. Direct and wrapped custom-destructor Map values are move-only and cannot be cloned or retrieved. Custom-drop resources may also be nested through ordinary structs inside move-only Vec elements, including Vec fields in an owning wrapper struct; Map values may likewise wrap these resources through ordinary struct fields, and a normal struct may own such a Map field; cloning, indexed reads, extraction, and slices remain rejected for those Vecs.

```ryn
#[drop(release_handle)]
struct Handle { raw: *u8 }

fun release_handle(handle: Handle) {
    unload_resource(handle.raw)
}
```

Treat the destructor function as a consuming implementation detail: Guard calls it automatically, and source code cannot call it directly. The destructor body must release the resource represented by the handle; the compiler cannot verify that a platform resource was actually released.

The compiler initializes every owning slot to an empty handle, including slots in branches that are not executed. Cleanup can therefore handle branch-dependent initialization and partial moves safely. Runtime failures terminate the process without stack unwinding; generated cleanup calls run before process termination, including registered custom destructors.

## Dynamic C symbols

`load_library(path) -> *u8`, `load_symbol(library, name) -> *u8`, `pointer_is_null(pointer)`, and `unload_library(library)` expose the host dynamic loader. Function pointer aliases such as `type AbsFn = extern "C" fun(i32) -> i32` may be called indirectly after casting a raw symbol pointer. Function pointer values can be passed as typed parameters and invoked indirectly. Current calls support scalar and pointer parameters/results and use the host default calling convention; aggregate arguments/results and comprehensive ABI validation are not implemented. Dynamic loading uses `load_library`, `load_symbol`, `pointer_is_null`, and `unload_library`; failed loads/lookups return null. This remains an unsafe boundary: ensure the symbol has exactly the declared ABI/signature, keep the library alive through the call, and unload each successful handle once.

The representation and helper C ABI are bootstrap implementation details. They do not establish a public `repr(C)` layout for String.

## UTF-8 String operations

`String()` constructs an empty owner; `String(text)` copies a `str` view into an owner. Appending and pushing grow the existing buffer geometrically, so an empty mutable String serves as a string builder.

| Operation | Result and indexing rule |
| --- | --- |
| `value.len()` | `u64` byte length |
| `value.char_count()` | `u64` Unicode scalar count |
| `value.byte_at(index)` | `u8`, indexed by byte |
| `value.char_at(index)` | `char`, indexed by Unicode scalar, not grapheme cluster |
| `value.slice(start, end)` | New owning String copied from an exclusive UTF-8 byte range |
| `value.find(pattern)` | `Option<u64>` Unicode scalar index of the first match, or `Option::None` if absent |
| `value.split(separator)` | Owning `Vec<String>` for a `str` or `String` separator; uses Rust UTF-8 split semantics, including empty fields |
| `value.clone()` | Independent owning copy |
| `value.trim()` | Independent String with Unicode whitespace trimmed |
| `value.concat(text)` | New String combining this String and a `str` or String |
| `value.starts_with(text)` / `ends_with(text)` / `contains(text)` | `bool`; pattern may be `str` or String |
| `value.append(text)` | Mutates this String; pattern may be `str` or String |
| `value.push(character)` | Appends one Unicode scalar |
| `value.clear()` | Removes contents while retaining reusable capacity |

Out-of-bounds indexing and slicing at a non-UTF-8 boundary produce a runtime diagnostic and terminate with status 1. Character literals support one Unicode scalar and escapes such as `'\n'`, `'\''`, and `'\u{1F980}'`. `char` supports equality and ordering by scalar value.

`String(number)` formats all integer and float widths. `to_i8` through `to_i64`, `to_u8` through `to_u64`, `to_f32`, and `to_f64` parse the complete String; invalid or out-of-range input prints a diagnostic and terminates with status 1. Their `try_to_i8` through `try_to_i64`, `try_to_u8` through `try_to_u64`, `try_to_f32`, and `try_to_f64` counterparts return `Option<T>` for parse failures. `find` returns an owning Option containing the Unicode scalar index; the runtime sentinel is translated before it reaches Ryn code. Unicode scalar iteration is supported directly for both String and str.

`char_from_u32(value)` constructs a `char` from a Unicode scalar value and terminates with a runtime diagnostic for values outside the Unicode scalar range (including surrogate code points). It is used by the Ryn-written lexer to decode `\u{...}` escapes; callers parsing untrusted numbers should validate the scalar first or wait for a fallible conversion API.

## Enums and pattern matching

Enums use tagged, heap-backed values. A declaration can contain unit variants and tuple-style payloads:

```ryn
enum Data { Count(i32), Text(String), Empty }

fun measure(value: Data) -> u64 {
    choose value {
        Data::Count(number) => number as u64,
        Data::Text(text) => text.char_count(),
        Data::Empty => 0 as u64
    }
}
```

`choose` must cover every variant or provide one `_` arm. A `choose` consumes an owning enum value. Payload bindings are local to their arm; an owned payload is moved out of the enum before the enum's remaining payload is destroyed. `String`, collections, and nested enum payloads are cleaned up on each arm's exit, including when the selected result moves one of those values onward. Generic enums and nested enum payloads have basic monomorphized support; payload combinations remain bounded by the layouts and generic-specialization limits in the feature audit. Enum payload layouts and tags are compiler/runtime implementation details.
