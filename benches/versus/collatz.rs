fn main() {
    let limit: u64 = 3_000_000;
    let (mut best, mut best_start) = (0u64, 0u64);
    let mut start: u64 = 1;
    while start < limit {
        let mut x = start; let mut steps = 0u64;
        while x != 1 { if x % 2 == 0 { x /= 2; } else { x = 3 * x + 1; } steps += 1; }
        if steps > best { best = steps; best_start = start; }
        start += 1;
    }
    println!("{}", best_start);
}
