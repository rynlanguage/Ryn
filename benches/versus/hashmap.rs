use std::collections::HashMap;
fn main() {
    let n: u64 = 2_000_000;
    let mut table: HashMap<u64, u64> = HashMap::new();
    let mut i: u64 = 0;
    while i < n { table.insert(i.wrapping_mul(2654435761) % 1000003 + i, i); i += 1; }
    let mut sum: u64 = 0;
    i = 0;
    while i < n { if let Some(v) = table.get(&(i.wrapping_mul(2654435761) % 1000003 + i)) { sum += *v; } i += 1; }
    println!("{}", sum);
    println!("{}", table.len());
}
