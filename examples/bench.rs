//! cargo run --release --example bench -- <dir> [runs]
use std::time::Instant;

use kestrel::listing::list_dir;
use kestrel::sort::{Sort, SortKey, sorted_order};

fn main() {
    let dir = std::env::args().nth(1).expect("usage: bench <dir> [runs]");
    let runs: u32 = std::env::args().nth(2).map_or(5, |r| r.parse().unwrap());
    for run in 1..=runs {
        let t = Instant::now();
        let snap = list_dir(dir.as_ref()).unwrap();
        let listed = t.elapsed();
        let t = Instant::now();
        let order =
            sorted_order(&snap, Sort { key: SortKey::Name, descending: false }, true, &Default::default());
        let sorted = t.elapsed();
        println!("run {run}: {} items, list {listed:?}, sort {sorted:?}", order.len());
    }
}
