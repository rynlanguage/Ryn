fn main() {
    let n: u64 = 20_000_000;
    let mut composite: Vec<bool> = Vec::new();
    let mut i: u64 = 0;
    while i <= n { composite.push(false); i += 1; }
    let mut p: u64 = 2;
    while p * p <= n {
        if !composite[p as usize] {
            let mut j = p * p;
            while j <= n { composite[j as usize] = true; j += p; }
        }
        p += 1;
    }
    let mut count: u64 = 0;
    let mut k: u64 = 2;
    while k <= n { if !composite[k as usize] { count += 1; } k += 1; }
    println!("{}", count);
}
