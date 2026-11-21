# generate: test dataset generator

    cargo build --release
    target/release/generate --seed 1 --events 1000000 --clicks 250000 --output dev
