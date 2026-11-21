//! Prints the three per-seed ID keys the generator draws in `Dataset::plan`:
//!
//!     genkeys --seed N
//!     user=0x.. event=0x.. click=0x..
//!
//! The generator's own sources are compiled in, so the keys cannot drift from
//! what `generate` writes. Only the low 40 bits matter (`scramble` in text.rs
//! is a bijection on 40 bits), so each key is printed masked to 40 bits.
//! The keys are drawn before anything size-dependent, so the sizes passed to
//! `plan` below do not change them.
#![allow(dead_code)]

#[path = "../../gen/src/random.rs"]
mod random;
#[path = "../../gen/src/text.rs"]
mod text;
#[path = "../../gen/src/plan.rs"]
mod plan;

use std::process::ExitCode;

const MASK: u64 = (1 << 40) - 1;

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let seed = match arguments.as_slice() {
        [flag, value] if flag == "--seed" => value.parse::<u64>().ok(),
        _ => None,
    };
    let Some(seed) = seed else {
        eprintln!("usage: genkeys --seed N");
        return ExitCode::from(2);
    };
    let dataset = plan::Dataset::plan(seed, 1_000, 250);
    println!(
        "user=0x{:x} event=0x{:x} click=0x{:x}",
        dataset.user_key & MASK,
        dataset.event_key & MASK,
        dataset.click_key & MASK
    );
    ExitCode::SUCCESS
}
