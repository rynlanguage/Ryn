struct Node { value: u64, next: Option<Box<Node>> }
fn main() {
    let n: u64 = 5_000_000;
    let mut head: Option<Box<Node>> = None;
    for i in 0..n { head = Some(Box::new(Node { value: i, next: head })); }
    let mut sum: u64 = 0;
    let mut at = &head;
    while let Some(node) = at { sum += node.value; at = &node.next; }
    println!("{}", sum);
    let mut cur = head;
    let mut freed: u64 = 0;
    while let Some(mut node) = cur { cur = node.next.take(); freed += 1; }
    println!("{}", 0 * freed);
}
