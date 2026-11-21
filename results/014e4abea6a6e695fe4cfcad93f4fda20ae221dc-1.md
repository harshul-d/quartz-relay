# Benchmark 014e4abea6a6e695fe4cfcad93f4fda20ae221dc

- CPU: Intel(R) Xeon(R) 6975P-C; 4 logical CPUs; x86-64-v3 flags all present; AVX-512: yes
- Data: `big`, 1.89 GB; 5 repetitions; docker flags of scripts/docker_run.sh with --cpus 4 --memory 24g
- Transparent huge pages: runner default `madvise`, defrag `madvise`
- Variants: `v9` = v9; `head-late` = HEAD PIPELINE_EARLY_WRITE=0; `head` = HEAD

## Wall time, seconds: median (min-max)

| Run | cold | warm |
| --- | --- | --- |
| v9 | 3.563 (3.523-4.310) | 1.677 (1.666-1.693) |
| head-late | 3.573 (3.554-4.317) | 1.677 (1.674-1.690) |
| head | 3.540 (3.512-4.301) | 1.669 (1.639-1.680) |
| v9 /bin/true | 0.518 (0.498-0.542) | 0.102 (0.093-0.105) |
| head-late /bin/true | 0.506 (0.489-0.552) | 0.105 (0.101-0.111) |
| head /bin/true | 0.511 (0.502-0.545) | 0.095 (0.089-0.106) |
| v9 native (warm) | - | 1.555 (1.550-1.578) |
| head-late native (warm) | - | 1.560 (1.551-1.591) |
| head native (warm) | - | 1.562 (1.545-1.583) |
| cat of all input, 4 readers | 3.085 (3.028-3.093) | 0.025 (0.025-0.025) |
| cat of all input, 16 readers | 3.921 (3.916-3.923) | 0.028 (0.027-0.028) |

## Phases of `v9` (seconds, median over runs)

| Phase | container cold | container warm | native warm |
| --- | --- | --- | --- |
| lookups | 0.012 | 0.009 | 0.009 |
| ID linking | 0.094 | 0.094 | 0.095 |
| events read+parse | 1.996 | 0.824 | 0.813 |
| dedup | 0.099 | 0.097 | 0.097 |
| clicks read+parse | 0.401 | 0.123 | 0.122 |
| sessionize+attribution | 0.367 | 0.365 | 0.365 |
| output writing | 0.046 | 0.045 | 0.046 |
| report+drops | 0.000 | 0.000 | 0.000 |
| outside main (start, exit, container) | 0.545 | 0.114 | 0.013 |
| start: run start -> main | 0.506 | 0.077 | 0.006 |
| exit: end of main -> run end | 0.040 | 0.035 | 0.006 |
| cpu-s read (all threads) | 6.140 | 0.256 | 0.235 |
| cpu-s group-events (all threads) | 0.657 | 0.659 | 0.653 |
| cpu-s group-clicks (all threads) | 0.249 | 0.246 | 0.249 |
| cpu-s sessions (all threads) | 0.169 | 0.170 | 0.170 |
| cpu-s attribution (all threads) | 0.377 | 0.378 | 0.376 |

Native `/usr/bin/time -v` of `v9` (median):

| max RSS MiB | minor faults | major faults | voluntary switches | involuntary switches | user s | system s |
| --- | --- | --- | --- | --- | --- | --- |
| 2057.72 | 2278.00 | 25.00 | 55.00 | 947.00 | 5.24 | 0.75 |

## Phases of `head-late` (seconds, median over runs)

| Phase | container cold | container warm | native warm |
| --- | --- | --- | --- |
| lookups | 0.013 | 0.009 | 0.010 |
| ID linking | 0.096 | 0.094 | 0.092 |
| events read+parse | 2.016 | 0.824 | 0.809 |
| dedup | 0.098 | 0.097 | 0.097 |
| clicks read+parse | 0.402 | 0.123 | 0.123 |
| sessionize+attribution | 0.372 | 0.371 | 0.372 |
| output writing | 0.048 | 0.047 | 0.047 |
| report+drops | 0.000 | 0.000 | 0.000 |
| outside main (start, exit, container) | 0.538 | 0.119 | 0.012 |
| start: run start -> main | 0.500 | 0.084 | 0.006 |
| exit: end of main -> run end | 0.040 | 0.036 | 0.006 |
| cpu-s read (all threads) | 6.188 | 0.252 | 0.232 |
| cpu-s group-events (all threads) | 0.678 | 0.675 | 0.679 |
| cpu-s group-clicks (all threads) | 0.251 | 0.250 | 0.251 |
| cpu-s sessions (all threads) | 0.169 | 0.170 | 0.171 |
| cpu-s attribution (all threads) | 0.380 | 0.379 | 0.379 |

Native `/usr/bin/time -v` of `head-late` (median):

| max RSS MiB | minor faults | major faults | voluntary switches | involuntary switches | user s | system s |
| --- | --- | --- | --- | --- | --- | --- |
| 2051.68 | 2332.00 | 27.00 | 58.00 | 819.00 | 5.27 | 0.76 |

## Phases of `head` (seconds, median over runs)

| Phase | container cold | container warm | native warm |
| --- | --- | --- | --- |
| lookups | 0.013 | 0.009 | 0.010 |
| ID linking | 0.096 | 0.094 | 0.091 |
| events read+parse | 1.992 | 0.815 | 0.814 |
| dedup | 0.099 | 0.097 | 0.097 |
| clicks read+parse | 0.401 | 0.123 | 0.124 |
| sessionize+attribution | 0.405 | 0.408 | 0.406 |
| output writing | 0.004 | 0.002 | 0.004 |
| report+drops | 0.000 | 0.000 | 0.000 |
| outside main (start, exit, container) | 0.527 | 0.115 | 0.013 |
| start: run start -> main | 0.491 | 0.080 | 0.006 |
| exit: end of main -> run end | 0.035 | 0.032 | 0.006 |
| cpu-s read (all threads) | 6.198 | 0.254 | 0.231 |
| cpu-s group-events (all threads) | 0.675 | 0.676 | 0.678 |
| cpu-s group-clicks (all threads) | 0.249 | 0.249 | 0.248 |
| cpu-s sessions (all threads) | 0.145 | 0.143 | 0.145 |
| cpu-s attribution (all threads) | 0.377 | 0.379 | 0.380 |

Native `/usr/bin/time -v` of `head` (median):

| max RSS MiB | minor faults | major faults | voluntary switches | involuntary switches | user s | system s |
| --- | --- | --- | --- | --- | --- | --- |
| 2033.89 | 2325.00 | 27.00 | 59.00 | 837.00 | 5.24 | 0.76 |

## Output checks (20M)

- head-late vs v9: MATCH
- head vs v9: MATCH

<details><summary>lscpu</summary>

```
Architecture:                            x86_64
CPU op-mode(s):                          32-bit, 64-bit
Address sizes:                           48 bits physical, 48 bits virtual
Byte Order:                              Little Endian
CPU(s):                                  4
On-line CPU(s) list:                     0-3
Vendor ID:                               GenuineIntel
Model name:                              Intel(R) Xeon(R) 6975P-C
CPU family:                              6
Model:                                   173
Thread(s) per core:                      2
Core(s) per socket:                      2
Socket(s):                               1
Stepping:                                1
BogoMIPS:                                5400.00
Flags:                                   fpu vme de pse tsc msr pae mce cx8 apic sep mtrr pge mca cmov pat pse36 clflush mmx fxsr sse sse2 ss ht syscall nx pdpe1gb rdtscp lm constant_tsc arch_perfmon rep_good nopl xtopology nonstop_tsc cpuid aperfmperf tsc_known_freq pni pclmulqdq monitor vmx ssse3 fma cx16 pdcm pcid sse4_1 sse4_2 x2apic movbe popcnt tsc_deadline_timer aes xsave avx f16c rdrand hypervisor lahf_lm abm 3dnowprefetch cpuid_fault ssbd ibrs ibpb stibp ibrs_enhanced tpr_shadow flexpriority ept vpid ept_ad fsgsbase tsc_adjust bmi1 avx2 smep bmi2 erms invpcid avx512f avx512dq rdseed adx smap avx512ifma clflushopt clwb avx512cd sha_ni avx512bw avx512vl xsaveopt xsavec xgetbv1 xsaves avx_vnni avx512_bf16 wbnoinvd ida arat vnmi avx512vbmi umip pku ospke waitpkg avx512_vbmi2 gfni vaes vpclmulqdq avx512_vnni avx512_bitalg tme avx512_vpopcntdq rdpid bus_lock_detect cldemote movdiri movdir64b md_clear serialize amx_bf16 avx512_fp16 amx_tile amx_int8 flush_l1d arch_capabilities
Virtualization:                          VT-x
Hypervisor vendor:                       KVM
Virtualization type:                     full
L1d cache:                               96 KiB (2 instances)
L1i cache:                               128 KiB (2 instances)
L2 cache:                                4 MiB (2 instances)
L3 cache:                                480 MiB (1 instance)
NUMA node(s):                            1
NUMA node0 CPU(s):                       0-3
Vulnerability Gather data sampling:      Not affected
Vulnerability Ghostwrite:                Not affected
Vulnerability Indirect target selection: Not affected
Vulnerability Itlb multihit:             Not affected
Vulnerability L1tf:                      Not affected
Vulnerability Mds:                       Not affected
Vulnerability Meltdown:                  Not affected
Vulnerability Mmio stale data:           Not affected
Vulnerability Old microcode:             Not affected
Vulnerability Reg file data sampling:    Not affected
Vulnerability Retbleed:                  Not affected
Vulnerability Spec rstack overflow:      Not affected
Vulnerability Spec store bypass:         Mitigation; Speculative Store Bypass disabled via prctl
Vulnerability Spectre v1:                Mitigation; usercopy/swapgs barriers and __user pointer sanitization
Vulnerability Spectre v2:                Mitigation; Enhanced / Automatic IBRS; IBPB conditional; PBRSB-eIBRS Not affected; BHI BHI_DIS_S
Vulnerability Srbds:                     Not affected
Vulnerability Tsa:                       Not affected
Vulnerability Tsx async abort:           Not affected
Vulnerability Vmscape:                   Not affected
```

</details>

## Profile of HEAD (perf, native, warm; self time)

```
    84.27%  pipeline            
    13.26%  [kernel.kallsyms]   
     2.46%  libc.so.6           
     0.01%  [dm_snapshot]       
     0.01%  [vdso]              
     0.00%  [ena]               
     0.00%  ld-linux-x86-64.so.2

    18.18%  [.] pipeline::parse_events::{closure#1}::{closure#0}::{closure#0}
    15.88%  [.] std::sys::backtrace::__rust_begin_short_backtrace::<pipeline::par::run<pipeline::ThreadResult, pipeline::sessionize::{closure#0}, pipeline::sessionize::{closure#1}>::{closure#0}::{closure#0}::{closure#0}, pipeline::ThreadResult>
    13.77%  [.] std::sys::backtrace::__rust_begin_short_backtrace::<pipeline::par::run<pipeline::parse_events::State, pipeline::parse_events::{closure#0}, pipeline::parse_events::{closure#1}>::{closure#0}::{closure#0}::{closure#0}, pipeline::parse_events::State>
     9.34%  [.] pipeline::parse_events::flush
     8.04%  [k] kernel_init_pages
     4.94%  [.] std::sys::backtrace::__rust_begin_short_backtrace::<pipeline::par::run<alloc::vec::Vec<alloc::vec::Vec<u64>>, pipeline::dedup_events::{closure#0}, pipeline::dedup_events::{closure#1}>::{closure#0}::{closure#0}::{closure#0}, alloc::vec::Vec<alloc::vec::Vec<u64>>>
     3.97%  [.] pipeline::parse_clicks::{closure#1}::{closure#0}::{closure#0}
     3.58%  [k] rep_movs_alternative
     2.35%  [.] core::slice::sort::shared::smallsort::small_sort_general::<pipeline::PersonEvent, <[pipeline::PersonEvent]>::sort_unstable_by_key<i64, pipeline::sessionize::{closure#1}::{closure#0}>::{closure#0}>
     1.98%  [.] core::slice::sort::unstable::quicksort::quicksort::<pipeline::PersonEvent, <[pipeline::PersonEvent]>::sort_unstable_by_key<i64, pipeline::sessionize::{closure#1}::{closure#0}>::{closure#0}>
     1.71%  [.] std::sys::backtrace::__rust_begin_short_backtrace::<pipeline::par::run<pipeline::parse_clicks::State, pipeline::parse_clicks::{closure#0}, pipeline::parse_clicks::{closure#1}>::{closure#0}::{closure#0}::{closure#0}, pipeline::parse_clicks::State>
     1.60%  [.] pipeline::parse_clicks::flush
     1.42%  [.] std::sys::backtrace::__rust_begin_short_backtrace::<pipeline::par::run<(), pipeline::links::build::{closure#7}, pipeline::links::build::{closure#8}>::{closure#0}::{closure#0}::{closure#0}, ()>
     1.19%  [.] <pipeline::out::Writer>::row_group
     1.06%  [.] pipeline::out::write_user_daily
     1.05%  [.] 0x00000000001bc4bf
     0.95%  [.] <hashbrown::map::HashMap<u128, u16, core::hash::BuildHasherDefault<pipeline::key::FastHasher>>>::get::<u128>
     0.83%  [.] 0x00000000001bc512
     0.76%  [.] core::slice::sort::unstable::quicksort::quicksort::<(u32, i64), <(u32, i64) as core::cmp::PartialOrd>::lt>
     0.73%  [.] core::slice::sort::shared::smallsort::small_sort_general::<pipeline::PersonClick, <[pipeline::PersonClick]>::sort_unstable_by<pipeline::sessionize::{closure#1}::{closure#1}>::{closure#0}>
     0.62%  [.] core::slice::sort::unstable::quicksort::quicksort::<pipeline::PersonClick, <[pipeline::PersonClick]>::sort_unstable_by<pipeline::sessionize::{closure#1}::{closure#1}>::{closure#0}>
     0.52%  [.] core::slice::sort::shared::smallsort::insertion_sort_shift_left::<pipeline::PersonClick, <[pipeline::PersonClick]>::sort_unstable_by<pipeline::sessionize::{closure#1}::{closure#1}>::{closure#0}>
     0.43%  [.] std::sys::backtrace::__rust_begin_short_backtrace::<pipeline::par::run<alloc::vec::Vec<(usize, alloc::vec::Vec<u128>)>, pipeline::links::build::{closure#3}, pipeline::links::build::{closure#4}>::{closure#0}::{closure#0}::{closure#0}, alloc::vec::Vec<(usize, alloc::vec::Vec<u128>)>>
     0.42%  [.] core::slice::sort::shared::smallsort::insertion_sort_shift_left::<(u32, i64), <(u32, i64) as core::cmp::PartialOrd>::lt>
     0.31%  [.] std::sys::backtrace::__rust_begin_short_backtrace::<pipeline::par::run<(), pipeline::links::build::{closure#9}, pipeline::links::build::{closure#10}>::{closure#0}::{closure#0}::{closure#0}, ()>
     0.30%  [.] pipeline::links::build::{closure#1}::{closure#0}::{closure#0}
     0.30%  [.] std::sys::backtrace::__rust_begin_short_backtrace::<pipeline::par::run<(), pipeline::links::build::{closure#13}, pipeline::links::build::{closure#14}>::{closure#0}::{closure#0}::{closure#0}, ()>
     0.26%  [.] 0x00000000001bcad8
     0.25%  [.] <[pipeline::PersonClick]>::sort_unstable_by::<pipeline::sessionize::{closure#1}::{closure#1}>::{closure#0}
     0.18%  [.] pipeline::links::build
     0.14%  [k] _copy_to_iter
     0.14%  [.] core::slice::sort::shared::pivot::median3_rec::<pipeline::PersonEvent, <[pipeline::PersonEvent]>::sort_unstable_by_key<i64, pipeline::sessionize::{closure#1}::{closure#0}>::{closure#0}>
     0.13%  [k] prep_compound_page
     0.13%  [.] std::sys::backtrace::__rust_begin_short_backtrace::<pipeline::par::run<pipeline::links::build::Parsed, pipeline::links::build::{closure#0}, pipeline::links::build::{closure#1}>::{closure#0}::{closure#0}::{closure#0}, pipeline::links::build::Parsed>
     0.10%  [.] pipeline::ts::price
     0.09%  [.] core::slice::sort::shared::smallsort::sort8_stable::<(u32, i64), <(u32, i64) as core::cmp::PartialOrd>::lt>
     0.07%  [.] <pipeline::out::Dictionary>::grow
     0.07%  [.] 0x00000000001bc237
     0.06%  [k] __rmqueue_pcplist
     0.05%  [k] native_write_msr
     0.05%  [.] core::slice::sort::unstable::ipnsort::<pipeline::PersonEvent, <[pipeline::PersonEvent]>::sort_unstable_by_key<i64, pipeline::sessionize::{closure#1}::{closure#0}>::{closure#0}>
     0.04%  [k] dm_kcopyd_prepare_callback
     0.04%  [k] task_tick_fair
     0.04%  [.] pipeline::main
     0.04%  [k] filemap_get_read_batch
```

### Hottest source lines per symbol

```
  6.75  src/ts.rs:155  Some(((days * 24 + hour as i64) * 60 + minute as i64) * MINUTE + second as i64 * SECOND)
  3.61  src/ts.rs:131  worst |= (digit > 9) as u32;
  2.93  src/main.rs:354  for_each_record::<7>(bytes, |&[event_id, user, ts_text, ingest_text, event_type, product, price_text]| {
  2.79  src/ts.rs:0  
  2.34  src/main.rs:387  state.pending.push(Pending {
  2.03  src/ts.rs:151  if day as i32 > MONTH_START[index + 1] - start {
  1.67  num/uint_macros.rs:2719  intrinsics::wrapping_sub(self, rhs)
  1.33  num/uint_macros.rs:1122  if self < rhs {
  1.27  src/key.rs:66  (word & !(u128::MAX >> (8 * length))) | length as u128
  1.10  src/key.rs:57  if length > 15 {
  0.91  src/key.rs:136  let product = (a as u128) * (b as u128);
  0.87  src/key.rs:137  (product as u64) ^ ((product >> 64) as u64)
  0.87  src/main.rs:358  let event_id = interner.key_in(bytes, event_id);
  0.85  src/ts.rs:150  let start = MONTH_START[index];
  0.73  src/main.rs:369  let purchase = match event_type {
  0.57  src/ts.rs:218  25 if (bytes[19] == b'+' || bytes[19] == b'-') && bytes[22] == b':' => {
  0.57  slice/index.rs:238  &(*slice)[self]
  0.51  src/ts.rs:136  let year = digits[0] * 1000 + digits[1] * 100 + digits[2] * 10 + digits[3];
  0.51  src/ts.rs:149  let index = year_index * 12 + month as usize - 1;
  0.51  src/ts.rs:216  match bytes.len() {
--- (lines above: one symbol, 3541 samples)
  5.87  src/main.rs:626  if event.flags & DELETED == 0 {
  5.40  src/hint.rs:879  crate::intrinsics::select_unpredictable(condition, true_val, false_val).assume_init()
  5.34  src/main.rs:836  if self.keys[held as usize - 1] == key {
  3.80  src/main.rs:954  while first < last {
  3.76  src/main.rs:603  if event.flags & DELETED != 0 {
  2.44  src/key.rs:146  let first = fold_mul(low ^ 0x243F_6A88_85A3_08D3, 0x9E37_79B9_7F4A_7C15);
  2.25  src/main.rs:628  spare[cursor[id]].write(*event);
  2.03  ptr/non_null.rs:1663  self.as_ptr() == other.as_ptr()
  2.03  x86/rdtsc.rs:26  rdtsc()
  2.00  src/main.rs:830  let held = self.slots[slot];
  1.80  src/main.rs:715  if event.flags & PURCHASE != 0 {
  1.80  src/main.rs:957  let end = first + starts[first..last].partition_point(|&s| s < next_day);
  1.60  shared/smallsort.rs:566  if !is_less(&tmp, &*sift) {
  1.51  src/main.rs:918  counts[0] += 1;
  1.28  src/main.rs:851  if self.keys[held as usize - 1] == key {
  1.03  src/main.rs:629  cursor[id] += 1;
  0.87  vec/mod.rs:3080  self.len() == 0
  0.80  vec/mod.rs:999  if len == self.buf.capacity() {
  0.70  ptr/mod.rs:574  unsafe { crate::intrinsics::copy_nonoverlapping(src, dst, count) }
  0.64  num/int_macros.rs:3214  if self % rhs < 0 {
  0.61  mem/maybe_uninit.rs:575  *self = MaybeUninit::new(val);
  0.58  slice/mod.rs:2987  if size == 0 {
  0.58  src/main.rs:828  let mut slot = hash(key) as usize & self.mask;
  0.55  shared/smallsort.rs:560  if sift == begin {
  0.54  src/key.rs:137  (product as u64) ^ ((product >> 64) as u64)
--- (lines above: one symbol, 3122 samples)
  6.71  ptr/mod.rs:574  unsafe { crate::intrinsics::copy_nonoverlapping(src, dst, count) }
  5.59  src/csv.rs:0  
  4.22  src/csv.rs:173  spans[count] = (field_start, at);
  3.73  src/csv.rs:134  let last = bytes[end.saturating_sub(1).min(bytes.len() - 1)];
  3.23  num/uint_macros.rs:178  return intrinsics::cttz(self);
  2.65  src/csv.rs:168  while separators != 0 {
  2.50  src/csv.rs:135  let quoted = (long_enough & (first == b'"') & (last == b'"')) as usize;
  2.46  src/csv.rs:171  let at = base + bit as usize;
  2.35  src/csv.rs:177  if (newline >> bit) & 1 != 0 {
  2.31  src/csv.rs:83  l | (h << 32)
  1.78  num/uint_macros.rs:85  return intrinsics::ctpop(self);
  1.57  src/csv.rs:172  if count < N {
  1.19  src/csv.rs:175  count += 1;
  1.08  src/csv.rs:136  values[k] = &bytes[start + quoted..end - quoted];
  0.67  src/cmp.rs:2311  crate::intrinsics::integer_min(self, other)
  0.67  src/csv.rs:159  let block: &[u8; 64] = if base + 64 <= length {
  0.52  src/csv.rs:133  let first = bytes[start.min(end.saturating_sub(1)).min(bytes.len() - 1)];
--- (lines above: one symbol, 2684 samples)
 23.96  ptr/mod.rs:1966  intrinsics::write_via_move(dst, src)
  9.50  src/links.rs:87  let (key, name) = self.table[slot];
  7.91  ptr/non_null.rs:1663  self.as_ptr() == other.as_ptr()
  7.86  src/main.rs:338  buffer.push(PersonEvent { person, ts: row.ts, price: row.price, category: row.category, flags: row.flags });
  6.21  slice/index.rs:238  &(*slice)[self]
  5.21  slice/index.rs:0  
  4.77  src/links.rs:74  self.filter[(bit >> 6) as usize] & (1 << (bit & 63)) != 0
  2.97  vec/mod.rs:999  if len == self.buf.capacity() {
  1.70  src/links.rs:79  prefetch(&self.table[(hashed >> (64 - self.table_bits)) as usize]);
  1.65  slice/index.rs:245  &mut (*slice)[self]
  1.37  raw_vec/mod.rs:635  self.ptr.cast()
  1.37  ptr/mut_ptr.rs:971  unsafe { intrinsics::offset(self, count) }
  1.27  src/links.rs:91  if key == 0 {
  1.16  src/key.rs:147  fold_mul(high ^ first, 0xD6E8_FEB8_6659_FD93)
  0.83  src/links.rs:125  hashed.rotate_left(29) & mask
  0.77  src/key.rs:137  (product as u64) ^ ((product >> 64) as u64)
  0.66  src/main.rs:329  if row.linked {
  0.66  src/main.rs:328  row.linked = names.may_be_linked(row.hashed);
  0.61  intrinsics/mod.rs:2142  unsafe { unchecked_funnel_shl(x, x, shift % (mem::size_of::<T>() as u32 * 8)) }
  0.60  src/links.rs:85  let mut slot = (hashed >> (64 - self.table_bits)) as usize;
  0.55  src/key.rs:136  let product = (a as u128) * (b as u128);
  0.55  vec/mod.rs:3055  let len = self.len;
--- (lines above: one symbol, 1820 samples)
 39.42  src/main.rs:452  if held == 0 {
 33.66  src/main.rs:457  if kept.event_id == copy.event_id {
  4.12  src/main.rs:449  let mut slot = hash(copy.event_id) as usize & mask;
  3.13  src/main.rs:0  
  2.43  slice/index.rs:238  &(*slice)[self]
  2.23  src/key.rs:147  fold_mul(high ^ first, 0xD6E8_FEB8_6659_FD93)
  1.93  src/main.rs:458  if copy.ingest < kept.ingest {
  1.72  vec/mod.rs:999  if len == self.buf.capacity() {
  1.72  src/key.rs:136  let product = (a as u128) * (b as u128);
  1.50  src/main.rs:456  let kept = &all[held as usize - 1];
  0.92  vec/mod.rs:0  
  0.92  src/key.rs:137  (product as u64) ^ ((product >> 64) as u64)
  0.71  src/main.rs:451  let held = slots[slot];
  0.70  ptr/non_null.rs:1663  self.as_ptr() == other.as_ptr()
--- (lines above: one symbol, 992 samples)
 10.42  src/cmp.rs:2176  fn eq(&self, other: &Self) -> bool { *self == *other }
  7.24  x86/sse2.rs:1570  simd_bitmask::<_, u16>(m) as u32 as i32
  3.24  src/ts.rs:0  
  2.83  src/key.rs:66  (word & !(u128::MAX >> (8 * length))) | length as u128
  2.71  src/ts.rs:131  worst |= (digit > 9) as u32;
  2.69  src/main.rs:514  for_each_record::<4>(bytes, |&[click_id, user, campaign, ts_text]| {
  2.45  ptr/mut_ptr.rs:1055  unsafe { intrinsics::offset(self, intrinsics::unchecked_sub(0, count as isize)) }
  2.07  num/uint_macros.rs:2719  intrinsics::wrapping_sub(self, rhs)
  1.93  src/ts.rs:217  20 if bytes[19] == b'Z' => civil_fast(bytes),
  1.68  src/ts.rs:216  match bytes.len() {
  1.67  num/uint_macros.rs:1122  if self < rhs {
  1.48  src/main.rs:0  
  1.41  src/ts.rs:180  while at + 8 <= bytes.len() {
  1.40  src/ts.rs:151  if day as i32 > MONTH_START[index + 1] - start {
  1.36  src/ts.rs:155  Some(((days * 24 + hour as i64) * 60 + minute as i64) * MINUTE + second as i64 * SECOND)
  1.29  src/key.rs:57  if length > 15 {
  1.16  src/ts.rs:170  Some(((chunk & 0x0000_FFFF_0000_0000) >> 32) + (chunk & 0x0000_0000_0000_FFFF) * 10_000)
  1.15  src/key.rs:147  fold_mul(high ^ first, 0xD6E8_FEB8_6659_FD93)
  1.04  num/uint_macros.rs:773  intrinsics::bswap(self as $ActualT) as Self
  1.03  ptr/mod.rs:1966  intrinsics::write_via_move(dst, src)
  1.03  src/ts.rs:218  25 if (bytes[19] == b'+' || bytes[19] == b'-') && bytes[22] == b':' => {
  0.91  intrinsics/mod.rs:2142  unsafe { unchecked_funnel_shl(x, x, shift % (mem::size_of::<T>() as u32 * 8)) }
  0.77  src/main.rs:519  if ts < window.click_start || ts >= window.end {
  0.65  src/ts.rs:169  let chunk = ((chunk & 0x00FF_0000_00FF_0000) >> 16) + (chunk & 0x0000_00FF_0000_00FF) * 100;
  0.65  control/tag.rs:47  
--- (lines above: one symbol, 780 samples)
 22.61  ptr/mod.rs:574  unsafe { crate::intrinsics::copy_nonoverlapping(src, dst, count) }
 18.08  shared/smallsort.rs:566  if !is_less(&tmp, &*sift) {
  9.10  src/cmp.rs:2215  fn lt(&self, other: &Self) -> bool { *self <  *other }
  6.07  shared/smallsort.rs:560  if sift == begin {
  5.22  shared/smallsort.rs:541  if !is_less(&*tail, &*sift) {
  4.98  ptr/mut_ptr.rs:971  unsafe { intrinsics::offset(self, count) }
  4.58  iter/range.rs:0  
  2.19  iter/range.rs:1103  if self.start < self.end {
  1.95  ptr/const_ptr.rs:872  unsafe { intrinsics::offset(self, count) }
  1.31  ptr/const_ptr.rs:474  unsafe { intrinsics::arith_offset(self, count) }
  1.30  ptr/mod.rs:0  
  0.87  shared/smallsort.rs:808  let mut right_rev = src.add(len - 1);
  0.87  shared/smallsort.rs:698  let is_l = !is_less(&*right_src, &*left_src);
  0.65  ptr/mod.rs:1758  crate::intrinsics::read_via_copy(src)
--- (lines above: one symbol, 460 samples)
 27.98  unstable/quicksort.rs:308  while state.right < unroll_end {
 24.92  unstable/quicksort.rs:284  state.num_lt += right_is_lt as usize;
 17.22  src/cmp.rs:2215  fn lt(&self, other: &Self) -> bool { *self <  *other }
 16.45  ptr/mod.rs:665  crate::intrinsics::copy(src, dst, count)
  3.36  ptr/mut_ptr.rs:971  unsafe { intrinsics::offset(self, count) }
  1.81  unstable/quicksort.rs:0  
  1.29  unstable/quicksort.rs:322  state.right = if is_done { state.gap.value } else { state.right };
  1.02  shared/smallsort.rs:166  small_sort_general(v, is_less);
  0.77  shared/pivot.rs:28  let len_div_8 = len / 8;
  0.52  shared/pivot.rs:82  let x = is_less(a, b);
  0.51  unstable/quicksort.rs:21  pub(crate) fn quicksort<'a, T, F>(
--- (lines above: one symbol, 389 samples)
 12.91  src/csv.rs:135  let quoted = (long_enough & (first == b'"') & (last == b'"')) as usize;
  7.23  src/cmp.rs:2311  crate::intrinsics::integer_min(self, other)
  6.91  src/csv.rs:134  let last = bytes[end.saturating_sub(1).min(bytes.len() - 1)];
  6.29  src/csv.rs:136  values[k] = &bytes[start + quoted..end - quoted];
  5.71  src/csv.rs:0  
  5.71  num/uint_macros.rs:1122  if self < rhs {
  5.41  src/csv.rs:83  l | (h << 32)
  4.70  src/csv.rs:173  spans[count] = (field_start, at);
  3.87  src/csv.rs:133  let first = bytes[start.min(end.saturating_sub(1)).min(bytes.len() - 1)];
  3.29  num/uint_macros.rs:2570  intrinsics::saturating_sub(self, rhs)
  2.99  src/csv.rs:130  let (start, end) = spans[k];
  2.71  num/uint_macros.rs:178  return intrinsics::cttz(self);
  1.81  ptr/mod.rs:574  unsafe { crate::intrinsics::copy_nonoverlapping(src, dst, count) }
  1.80  src/csv.rs:177  if (newline >> bit) & 1 != 0 {
  1.46  num/uint_macros.rs:85  return intrinsics::ctpop(self);
  1.20  slice/index.rs:90  let ptr = unsafe { crate::intrinsics::offset(ptr, offset) };
  1.19  src/csv.rs:172  if count < N {
  1.18  num/uint_macros.rs:0  
  0.91  src/csv.rs:142  each(&values);
  0.83  src/csv.rs:166  let mut separators = comma | newline;
  0.61  src/csv.rs:139  if wrapped != quotes {
  0.60  ptr/mod.rs:0  
  0.60  src/csv.rs:188  line_quotes += quote.count_ones() - counted;
  0.60  src/csv.rs:137  wrapped += 2 * quoted as u32;
--- (lines above: one symbol, 334 samples)
 15.07  ptr/mod.rs:1966  intrinsics::write_via_move(dst, src)
 14.74  src/links.rs:87  let (key, name) = self.table[slot];
  8.62  ptr/non_null.rs:1663  self.as_ptr() == other.as_ptr()
  8.59  vec/mod.rs:999  if len == self.buf.capacity() {
  7.66  slice/index.rs:238  &(*slice)[self]
  4.47  src/links.rs:74  self.filter[(bit >> 6) as usize] & (1 << (bit & 63)) != 0
  4.16  slice/index.rs:0  
  3.21  src/links.rs:91  if key == 0 {
  2.87  raw_vec/mod.rs:635  self.ptr.cast()
  2.87  src/main.rs:0  
  2.25  src/key.rs:147  fold_mul(high ^ first, 0xD6E8_FEB8_6659_FD93)
  2.25  src/links.rs:79  prefetch(&self.table[(hashed >> (64 - self.table_bits)) as usize]);
  2.24  slice/index.rs:245  &mut (*slice)[self]
  2.24  src/main.rs:495  *linked = names.may_be_linked(*hashed);
  2.22  vec/mod.rs:996  let len = self.len;
  2.10  src/links.rs:85  let mut slot = (hashed >> (64 - self.table_bits)) as usize;
  1.92  src/key.rs:136  let product = (a as u128) * (b as u128);
  1.60  src/key.rs:137  (product as u64) ^ ((product >> 64) as u64)
  0.97  ptr/mut_ptr.rs:971  unsafe { intrinsics::offset(self, count) }
  0.97  src/links.rs:125  hashed.rotate_left(29) & mask
  0.96  src/links.rs:94  slot = (slot & !self.range_mask) | ((slot + 1) & self.range_mask);
  0.65  src/key.rs:146  let first = fold_mul(low ^ 0x243F_6A88_85A3_08D3, 0x9E37_79B9_7F4A_7C15);
  0.65  iter/macros.rs:0  
  0.65  src/main.rs:500  for &(mut click, hashed, linked) in &state.pending {
  0.64  src/main.rs:501  if linked {
--- (lines above: one symbol, 313 samples)
 41.30  src/links.rs:140  if held == 0 {
 21.39  src/links.rs:145  if self.keys[id as usize] == key {
 10.45  src/links.rs:250  if grand != up {
  7.76  src/links.rs:267  if parent[high as usize].compare_exchange(high, low, Ordering::AcqRel, Ordering::Relaxed).is_ok() {
  5.76  src/links.rs:246  if up == x {
  3.24  src/links.rs:266  if compare(interner, names.keys[a as usize], names.keys[b as usize]).is_lt() { (a, b) } else { (b, a) };
  0.72  src/links.rs:257  for &(a, b) in &parsed[index].pairs {
  0.72  src/links.rs:139  let held = self.slots[base + slot];
  0.68  slice/index.rs:238  &(*slice)[self]
--- (lines above: one symbol, 290 samples)
 14.56  num/uint_macros.rs:3818  let d = self / rhs;
 14.27  src/out.rs:223  rle_size += varint_len(((at - start) as u64) << 1) + value_bytes;
 10.81  src/out.rs:220  while at < values.len() && values[at] == values[start] {
  8.20  src/out.rs:231  while at < values.len() && values[at] == values[start] {
  4.69  src/out.rs:234  varint(out, ((at - start) as u64) << 1);
  4.28  src/out.rs:221  at += 1;
  3.91  src/out.rs:232  at += 1;
  3.47  num/uint_macros.rs:3820  if r > 0 {
  2.99  src/out.rs:0  
  2.94  src/out.rs:353  packed[index / 8] |= (present as u8) << (index % 8);
  2.60  src/option.rs:2142  Some(&v) => Some(v),
  2.18  ptr/mod.rs:1966  intrinsics::write_via_move(dst, src)
  2.17  vec/mod.rs:999  if len == self.buf.capacity() {
  1.74  raw_vec/mod.rs:640  if elem_size == 0 { usize::MAX } else { self.cap.as_inner() }
  1.73  src/cmp.rs:2215  fn lt(&self, other: &Self) -> bool { *self <  *other }
  1.72  src/out.rs:244  while bits >= 8 {
  1.71  slice/index.rs:0  
  1.30  adapters/enumerate.rs:82  self.count += 1;
  1.30  iter/range.rs:1103  if self.start < self.end {
  1.30  src/out.rs:242  buffer |= (values.get(index).copied().unwrap_or(0) as u64) << bits;
  1.29  num/uint_macros.rs:154  return intrinsics::ctlz(self as $ActualT);
  0.87  vec/mod.rs:2938  self.len += count;
  0.87  vec/mod.rs:1006  self.len = len + 1;
  0.86  src/out.rs:247  bits -= 8;
--- (lines above: one symbol, 232 samples)
 16.02  src/out.rs:168  if index == 0 {
  7.57  src/out.rs:167  let (held, index) = self.slots[slot];
  6.99  slice/index.rs:238  &(*slice)[self]
  6.78  src/out.rs:161  if self.count as usize * 2 >= self.slots.len() {
  6.74  src/key.rs:136  let product = (a as u128) * (b as u128);
  4.46  num/uint_macros.rs:2719  intrinsics::wrapping_sub(self, rhs)
  4.34  src/out.rs:555  columns[0].dict_next_bytes(previous == Some(row.person), |out| strings.append(row.person, out));
  3.38  src/out.rs:175  slot = (slot + 1) & mask;
  1.94  src/out.rs:173  return index - 1;
  1.93  src/out.rs:165  let mut slot = crate::key::hash(key as u128) as usize & mask;
  1.93  src/out.rs:287  let index = if repeat && dict.count > 0 {
  1.92  vec/mod.rs:1006  self.len = len + 1;
  1.61  src/out.rs:164  let mask = self.slots.len() - 1;
  1.51  src/key.rs:146  let first = fold_mul(low ^ 0x243F_6A88_85A3_08D3, 0x9E37_79B9_7F4A_7C15);
  1.45  src/out.rs:557  columns[1].dict_int32(row.day);
  1.12  vec/mod.rs:999  if len == self.buf.capacity() {
  0.97  ptr/mod.rs:1966  intrinsics::write_via_move(dst, src)
  0.97  vec/mod.rs:2932  if count > 0 {
  0.97  ptr/non_null.rs:1663  self.as_ptr() == other.as_ptr()
  0.97  slice/index.rs:0  
  0.96  vec/mod.rs:3055  let len = self.len;
--- (lines above: one symbol, 214 samples)
 58.38  src/cmp.rs:2176  fn eq(&self, other: &Self) -> bool { *self == *other }
 28.65  x86/sse2.rs:1570  simd_bitmask::<_, u16>(m) as u32 as i32
  3.25  src/key.rs:147  fold_mul(high ^ first, 0xD6E8_FEB8_6659_FD93)
  2.16  src/map.rs:1284  
  1.62  src/raw.rs:2032  
  1.62  ptr/mut_ptr.rs:1055  unsafe { intrinsics::offset(self, intrinsics::unchecked_sub(0, count as isize)) }
  1.08  src/raw.rs:2452  
  1.08  src/raw.rs:1337  
  1.07  control/tag.rs:47  
  0.54  src/raw.rs:1217  
  0.54  ptr/mod.rs:574  unsafe { crate::intrinsics::copy_nonoverlapping(src, dst, count) }
--- (lines above: one symbol, 185 samples)
 29.42  src/tuple.rs:196  match PartialOrd::$chain_rel(&$a, &$b) {
 20.02  ptr/mod.rs:574  unsafe { crate::intrinsics::copy_nonoverlapping(src, dst, count) }
  9.97  ptr/const_ptr.rs:0  
  6.63  shared/smallsort.rs:566  if !is_less(&tmp, &*sift) {
  4.65  shared/smallsort.rs:0  
  3.94  src/cmp.rs:2230  if lhs == rhs { Continue(()) } else { Break(lhs < rhs) }
  3.33  ptr/mut_ptr.rs:971  unsafe { intrinsics::offset(self, count) }
  2.67  unstable/quicksort.rs:284  state.num_lt += right_is_lt as usize;
  2.65  shared/smallsort.rs:541  if !is_less(&*tail, &*sift) {
  2.00  ptr/mod.rs:665  crate::intrinsics::copy(src, dst, count)
  2.00  shared/smallsort.rs:560  if sift == begin {
  1.34  unstable/quicksort.rs:51  if !is_less(p, &v[pivot_pos]) {
  1.34  shared/smallsort.rs:698  let is_l = !is_less(&*right_src, &*left_src);
  1.34  shared/smallsort.rs:808  let mut right_rev = src.add(len - 1);
  1.34  src/cmp.rs:2215  fn lt(&self, other: &Self) -> bool { *self <  *other }
  1.32  ptr/const_ptr.rs:474  unsafe { intrinsics::arith_offset(self, count) }
  0.68  unstable/quicksort.rs:326  if is_done {
  0.67  shared/smallsort.rs:252  } else if len >= 8 {
  0.67  iter/range.rs:0  
  0.67  unstable/quicksort.rs:308  while state.right < unroll_end {
  0.67  unstable/quicksort.rs:80  }
  0.67  unstable/quicksort.rs:0  
  0.67  shared/smallsort.rs:822  let last_src = if left_nonempty { left } else { right };
  0.66  unstable/quicksort.rs:126  if num_lt >= len {
```
