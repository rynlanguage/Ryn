# Ryn 0.1.12: syntax sugar and game-development essentials

What `rync` accepts since 0.1.12, with what already existed marked as such. Everything here is checked by tests in
`tests/suite/sugar/` and `tests/suite/gamedev/`; `examples/sugar_rush.ryn` and `examples/gamedev_essentials.ryn` use most of
it and run as tests.

## Syntax sugar

| Feature | State | Notes |
|---|---|---|
| `for i in a..b`, `a..=b` | existed; typing improved | A range of literals takes the integer type the counter is used as in the body (`total += i`, `i < n`, `f(i)`, `v[i]`), so `for i in 0..5 { total += i }` works with `total: u64`. `for x in array` iterates a fixed array (a copy, so the array may change in the body). |
| String interpolation | **new for any string** | `"Hello {name}"` and `"{p.name} has {p.hp} hp"` are a `String` built at run time: each `{...}` is a variable or a field path of a type `String`, `str`, `bool`, `char` or a number. The literal is interpolated only when every name in braces is a variable in scope, so `"{}"`, `"{not_a_variable}"` and JSON text stay as they were; `String("{name}")` keeps its braces too. `echo` and `print` keep their own forms. Not interpolated: expressions (`{a + b}`), calls. The result allocates, as the braces say. |
| `when` as a value | existed | `x := when c { 1 } else when d { 2 } else { 3 }`. |
| Destructuring | existed for tuples and structures; **arrays new** | `(a, b) := pair`, `Point { x, y } := p`, `[a, b, c] := array`, nested in each other, `mut` and `_` allowed. |
| `?.` and `??` | existed | `item?.weight ?? 0`. |
| `\|>` | existed | `x \|> f \|> g(2)` is `g(f(x), 2)`. |
| Compound assignment | extended | `+= -= *= /= %= &= \|= ^= <<= >>=` on variables, fields, **array elements** (`a[i] += 1`, `self.m[i] *= 2`, `grid[y][x] -= 1`) and, with the operator method, **structures** (`pos += velocity`). The index of a compound element assignment must not contain a call (it is evaluated twice). |
| `choose` | extended | Integer and character **ranges** (`1..=5`, `0..10`, `..=-1`, `'a'..='z'`), **guards** (`x when x > 50 => ...`, also on variant arms: `Shape::Circle(r) when r > 10 => ...`), bindings, `_`. A guarded arm falls to the next arms when the guard is false and needs a later arm that covers the same values (R0235). Ranges must be followed by a `_` or binding arm, like literals. |
| `defer` | extended | `defer statement` as well as `defer { ... }`; deferred code runs last-first on every way out of the scope. |

## Game-development essentials

| Feature | State | Notes |
|---|---|---|
| Swizzles | **new** | `v.xy`, `v.zyx`, `c.rgb`, `c.xy` on a structure with fields `x y z w` (or `r g b a`, or `s t p q`; the sets alias each other by position). The result is the structure with that many components of the same type (`x y z w`, or `r g b a` when written with `rgba` letters): `Vec3.xy` needs a `Vec2 { x, y }` of the same component type. No allocation. Reads only; the value must be a variable or a field of one. |
| Fixed arrays | extended | `[f32; 16]`, `[[u8; 4]; 3]` live in the frame or in a structure, with no heap. New: `len()` is a constant; arrays in structures are read and **written through `self` and `&mut`** (`self.m[i] = v`), nested arrays are bounds-checked per dimension, an array of structures can be assigned element by element. |
| Operators for your types | extended | Methods `add sub mul div rem bitand bitor bitxor shl shr` give the binary operators, `neg` gives `-x`, `not` gives `!x`, `eq` gives `==` and `!=`, `lt le gt ge` give the comparisons, `index(self, i)` gives `x[i]`, and `+=`, `\|=` ... call the same methods. `rmul`, `radd`, `rsub`, `rdiv` give `number op value` (`2.0 * v`). An operator with no method is R0206 and names the method to write. |
| Layout | existed | `#[repr(C)]`, `#[repr(C, packed)]`, `#[repr(C, align(32))]`, `size_of::<T>()`, `align_of::<T>()`, `offset_of::<T>("field")` (also at run time). |
| Bit flags | **new** | `bitflags Layer: u8 { SOLID = 1, TRIGGER = 2, DEFAULT = SOLID \| TRIGGER, HIDDEN }` declares a structure with a `bits` field, the constants `Layer::SOLID`, ..., the operators `\| & ^ - !` (so `\|=` works), and `empty()`, `all()`, `from_bits(b)`, `contains`, `intersects`, `is_empty`, `is_all`, `insert`, `remove`, `toggle`, `set(flag, on)`. A flag without `= value` takes `1 << position`. The declaration is written out as ordinary Ryn after the file; its own text is blanked, so the positions of the rest of the file do not move. |
| RAII | existed | Destructors with `#[drop(...)]`, `defer`; nothing here adds a garbage collector or a hidden allocation: swizzles, operators, arrays and flags use no heap; only interpolation allocates. |

## Limits

* Interpolation takes names and field paths, not expressions. Use a variable for anything else.
* Swizzles are read-only (`v.xy = ...` is not supported) and need a structure of the result's shape to exist.
* `number op value` needs `rmul`-style methods; scalar types cannot be extended.
* `.step_by(n)` on ranges is not provided.
* A range pattern does not mark a later literal that it covers as unreachable.
* `@repr(C)` is written `#[repr(C)]`.
* Compound assignment of an element evaluates the index twice, so an index with a call is rejected.

## Fixed on the way

* `==` and `!=` of floats (and of structures with float fields) answered "equal" for any two ordered values: the second `setcc` overwrote the first. Tests in `tests/suite/sugar/float_equality_compares_values.ryn`.
* `String::concat(str)` leaked the copy of its argument; it is freed after the join.
