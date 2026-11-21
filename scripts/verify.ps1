# Full local check of the current build: unit tests, fixtures, dev answer,
# differential suite. Usage: powershell -File scripts/verify.ps1
$root = Split-Path -Parent (Split-Path -Parent $PSCommandPath)
$work = Split-Path -Parent $root
$bin = Join-Path $root "target\release\pipeline.exe"
Set-Location $root
cargo build --release 2>&1 | Select-Object -Last 1
cargo test --release 2>&1 | Select-String "test result|FAILED"
python scripts/run_fixtures.py fixtures -- $bin 2>&1 | Select-Object -Last 1
$out = Join-Path $work "data\out-rs-dev"
if (Test-Path $out) { Remove-Item -Recurse -Force $out }
$env:DATA_DIR = Join-Path $work "data\dev"; $env:OUTPUT_DIR = $out
& $bin 2>$null
Remove-Item Env:DATA_DIR, Env:OUTPUT_DIR
"dev: " + (python scripts/compare.py $out (Join-Path $work "dev-answer") | Select-Object -Last 1)
python scripts/diff_test.py --generator (Join-Path $work "generator\target\release\generate.exe") --binary $bin --data (Join-Path $work "data\diff") | Select-Object -Last 1
