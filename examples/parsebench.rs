//! Throwaway: single-thread cost of each parse stage on in-memory event data.
#[path = "../src/csv.rs"]
#[allow(dead_code)]
mod csv;
#[path = "../src/key.rs"]
#[allow(dead_code)]
mod key;
#[path = "../src/links.rs"]
#[allow(dead_code)]
mod links;
#[path = "../src/par.rs"]
mod par;
#[path = "../src/timing.rs"]
#[allow(dead_code)]
mod timing;
#[path = "../src/ts.rs"]
#[allow(dead_code)]
mod ts;

use std::hint::black_box;
use std::time::Instant;

fn main() {
    let path = std::env::args().nth(1).unwrap();
    let data = std::fs::read(&path).unwrap();
    let rows = memchr::memchr_iter(b'\n', &data).count();
    let interner = key::Interner::default();
    let report = |name: &str, t: Instant| {
        let s = t.elapsed().as_secs_f64();
        println!("{name:28} {:6.1} ns/row  {:6.0} MB/s", s * 1e9 / rows as f64, data.len() as f64 / s / 1e6);
    };
    let links_dir = std::path::Path::new(&path).parent().unwrap().parent().unwrap();
    let t = Instant::now();
    let names = links::build(links_dir, &interner, 8, 2 << 20);
    println!("links build {:?}", t.elapsed());
    for _ in 0..2 {
        let t = Instant::now();
        let mut n = 0u128;
        csv::for_each_record::<7>(&data, |f| n = n.wrapping_add(names.person(interner.key(f[1]))));
        black_box(n);
        report("tokenize + person", t);
        let t = Instant::now();
        let mut hits = 0u64;
        csv::for_each_record::<7>(&data, |f| { let u = interner.key(f[1]); hits += (names.person(u) != u) as u64; });
        println!("rows mapped to another name: {:.1}%", hits as f64 * 100.0 / rows as f64);
        black_box(t);
        let t = Instant::now();
        let mut n = 0u64;
        csv::for_each_record::<7>(&data, |f| n += f[0].len() as u64);
        black_box(n);
        report("tokenize", t);

        let t = Instant::now();
        let mut n = 0u64;
        csv::for_each_line(&data, |line| {
            if let Some(f) = csv::split::<7>(line) {
                n += f[0].len() as u64;
            }
        });
        black_box(n);
        report("simple split", t);

        let t = Instant::now();
        let mut n = 0i64;
        csv::for_each_record::<7>(&data, |f| {
            n = n.wrapping_add(ts::parse(f[2]).unwrap_or(0)).wrapping_add(ts::parse(f[3]).unwrap_or(0));
        });
        black_box(n);
        report("tokenize + 2 ts", t);

        let t = Instant::now();
        let mut n = 0u128;
        csv::for_each_record::<7>(&data, |f| {
            n = n.wrapping_add(interner.key(f[0])).wrapping_add(interner.key(f[1]));
        });
        black_box(n);
        report("tokenize + 2 keys", t);

        let t = Instant::now();
        let mut n = 0u64;
        csv::for_each_record::<7>(&data, |f| {
            n = n.wrapping_add(key::hash(interner.key(f[0]))).wrapping_add(key::hash(interner.key(f[1])));
        });
        black_box(n);
        report("tokenize + 2 keys + hash", t);

        #[derive(Clone, Copy)]
        #[allow(dead_code)]
        struct Rec(u128, i64, u64);
        let t = Instant::now();
        let mut parts: Vec<Vec<Rec>> = (0..256).map(|_| Vec::new()).collect();
        csv::for_each_record::<7>(&data, |f| {
            let k = interner.key(f[0]);
            parts[(key::hash(k) >> 56) as usize].push(Rec(k, 1, 2));
        });
        black_box(&parts);
        report("tokenize + 1 scatter (fresh)", t);
        drop(parts);

        let t = Instant::now();
        let mut parts: Vec<Vec<Rec>> = (0..256).map(|_| Vec::new()).collect();
        let mut parts2: Vec<Vec<Rec>> = (0..256).map(|_| Vec::new()).collect();
        csv::for_each_record::<7>(&data, |f| {
            let k = interner.key(f[0]);
            let u = interner.key(f[1]);
            parts[(key::hash(k) >> 56) as usize].push(Rec(k, 1, 2));
            parts2[(key::hash(u) >> 56) as usize].push(Rec(u, 1, 2));
        });
        black_box((&parts, &parts2));
        report("tokenize + 2 scatters (fresh)", t);
        for p in parts.iter_mut().chain(parts2.iter_mut()) {
            p.clear();
        }
        let t = Instant::now();
        csv::for_each_record::<7>(&data, |f| {
            let k = interner.key(f[0]);
            let u = interner.key(f[1]);
            parts[(key::hash(k) >> 56) as usize].push(Rec(k, 1, 2));
            parts2[(key::hash(u) >> 56) as usize].push(Rec(u, 1, 2));
        });
        black_box((&parts, &parts2));
        report("tokenize + 2 scatters (reused)", t);
    }
}
