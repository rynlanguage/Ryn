fn main() {
    let count: usize = 1048576;
    let mut data: Vec<f32> = Vec::with_capacity(count);
    for at in 0..count { data.push(((at % 7) as f32) * 0.5); }
    let mut total: f32 = 0.0;
    for _ in 0..64 {
        let mut partial: f32 = 0.0;
        for v in data.iter() { partial += *v; }
        total += partial;
    }
    println!("{}", total);
}
