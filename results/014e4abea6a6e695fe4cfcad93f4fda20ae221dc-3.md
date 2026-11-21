# Benchmark 014e4abea6a6e695fe4cfcad93f4fda20ae221dc

- CPU: INTEL(R) XEON(R) PLATINUM 8573C; 4 logical CPUs; x86-64-v3 flags all present; AVX-512: yes
- Data: `big`, 1.89 GB; 5 repetitions; docker flags of scripts/docker_run.sh with --cpus 4 --memory 24g
- Transparent huge pages: runner default `madvise`, defrag `madvise`
- Variants: `v9` = v9; `head-late` = HEAD PIPELINE_EARLY_WRITE=0; `head` = HEAD

## Wall time, seconds: median (min-max)

| Run | cold | warm |
| --- | --- | --- |
| v9 | 6.355 (5.387-8.018) | 2.062 (2.034-2.176) |
| head-late | 6.421 (5.357-7.815) | 2.080 (2.022-2.161) |
| head | 5.989 (5.360-7.088) | 1.987 (1.926-2.121) |
| v9 /bin/true | 0.369 (0.242-0.423) | 0.125 (0.124-0.130) |
| head-late /bin/true | 0.267 (0.255-0.420) | 0.132 (0.120-0.141) |
| head /bin/true | 0.302 (0.262-0.362) | 0.137 (0.128-0.140) |
| v9 native (warm) | - | 1.823 (1.805-1.829) |
| head-late native (warm) | - | 1.824 (1.801-1.832) |
| head native (warm) | - | 1.796 (1.788-1.811) |
| cat of all input, 4 readers | 4.950 (4.348-5.026) | 0.033 (0.033-0.033) |
| cat of all input, 16 readers | 4.521 (4.437-4.533) | 0.035 (0.034-0.037) |

## Phases of `v9` (seconds, median over runs)

| Phase | container cold | container warm | native warm |
| --- | --- | --- | --- |
| lookups | 0.320 | 0.013 | 0.011 |
| ID linking | 0.609 | 0.096 | 0.093 |
| events read+parse | 3.874 | 1.006 | 1.000 |
| dedup | 0.102 | 0.104 | 0.103 |
| clicks read+parse | 0.456 | 0.147 | 0.145 |
| sessionize+attribution | 0.404 | 0.402 | 0.400 |
| output writing | 0.051 | 0.051 | 0.051 |
| report+drops | 0.000 | 0.000 | 0.000 |
| outside main (start, exit, container) | 0.512 | 0.231 | 0.012 |
| start: run start -> main | 0.309 | 0.098 | 0.006 |
| exit: end of main -> run end | 0.123 | 0.132 | 0.006 |
| cpu-s read (all threads) | 14.429 | 0.244 | 0.226 |
| cpu-s group-events (all threads) | 0.718 | 0.715 | 0.715 |
| cpu-s group-clicks (all threads) | 0.274 | 0.267 | 0.268 |
| cpu-s sessions (all threads) | 0.192 | 0.192 | 0.193 |
| cpu-s attribution (all threads) | 0.422 | 0.422 | 0.422 |

Native `/usr/bin/time -v` of `v9` (median):

| max RSS MiB | minor faults | major faults | voluntary switches | involuntary switches | user s | system s |
| --- | --- | --- | --- | --- | --- | --- |
| 2053.80 | 2302.00 | 25.00 | 56.00 | 949.00 | 6.19 | 0.82 |

## Phases of `head-late` (seconds, median over runs)

| Phase | container cold | container warm | native warm |
| --- | --- | --- | --- |
| lookups | 0.078 | 0.011 | 0.014 |
| ID linking | 0.182 | 0.095 | 0.096 |
| events read+parse | 4.030 | 0.988 | 0.981 |
| dedup | 0.104 | 0.103 | 0.102 |
| clicks read+parse | 0.464 | 0.146 | 0.146 |
| sessionize+attribution | 0.407 | 0.404 | 0.404 |
| output writing | 0.053 | 0.053 | 0.054 |
| report+drops | 0.000 | 0.000 | 0.000 |
| outside main (start, exit, container) | 0.464 | 0.259 | 0.013 |
| start: run start -> main | 0.222 | 0.101 | 0.006 |
| exit: end of main -> run end | 0.127 | 0.143 | 0.007 |
| cpu-s read (all threads) | 14.643 | 0.246 | 0.221 |
| cpu-s group-events (all threads) | 0.724 | 0.720 | 0.722 |
| cpu-s group-clicks (all threads) | 0.274 | 0.273 | 0.272 |
| cpu-s sessions (all threads) | 0.192 | 0.192 | 0.194 |
| cpu-s attribution (all threads) | 0.425 | 0.427 | 0.424 |

Native `/usr/bin/time -v` of `head-late` (median):

| max RSS MiB | minor faults | major faults | voluntary switches | involuntary switches | user s | system s |
| --- | --- | --- | --- | --- | --- | --- |
| 2047.75 | 2292.00 | 27.00 | 62.00 | 922.00 | 6.18 | 0.82 |

## Phases of `head` (seconds, median over runs)

| Phase | container cold | container warm | native warm |
| --- | --- | --- | --- |
| lookups | 0.168 | 0.010 | 0.012 |
| ID linking | 0.338 | 0.094 | 0.091 |
| events read+parse | 4.028 | 0.988 | 0.982 |
| dedup | 0.103 | 0.103 | 0.102 |
| clicks read+parse | 0.467 | 0.146 | 0.146 |
| sessionize+attribution | 0.445 | 0.447 | 0.442 |
| output writing | 0.005 | 0.004 | 0.004 |
| report+drops | 0.000 | 0.000 | 0.000 |
| outside main (start, exit, container) | 0.255 | 0.180 | 0.012 |
| start: run start -> main | 0.212 | 0.140 | 0.006 |
| exit: end of main -> run end | 0.044 | 0.041 | 0.006 |
| cpu-s read (all threads) | 14.371 | 0.241 | 0.222 |
| cpu-s group-events (all threads) | 0.724 | 0.722 | 0.722 |
| cpu-s group-clicks (all threads) | 0.273 | 0.276 | 0.272 |
| cpu-s sessions (all threads) | 0.165 | 0.161 | 0.160 |
| cpu-s attribution (all threads) | 0.423 | 0.424 | 0.423 |

Native `/usr/bin/time -v` of `head` (median):

| max RSS MiB | minor faults | major faults | voluntary switches | involuntary switches | user s | system s |
| --- | --- | --- | --- | --- | --- | --- |
| 2031.75 | 2297.00 | 27.00 | 60.00 | 903.00 | 6.13 | 0.82 |

## Output checks (20M)

- head-late vs v9: MATCH
- head vs v9: MATCH

<details><summary>lscpu</summary>

```
Architecture:                            x86_64
CPU op-mode(s):                          32-bit, 64-bit
Address sizes:                           48 bits physical, 57 bits virtual
Byte Order:                              Little Endian
CPU(s):                                  4
On-line CPU(s) list:                     0-3
Vendor ID:                               GenuineIntel
Model name:                              INTEL(R) XEON(R) PLATINUM 8573C
CPU family:                              6
Model:                                   207
Thread(s) per core:                      2
Core(s) per socket:                      2
Socket(s):                               1
Stepping:                                2
CPU(s) scaling MHz:                      95%
CPU max MHz:                             2300.0000
CPU min MHz:                             800.0000
BogoMIPS:                                4599.99
Flags:                                   fpu vme de pse tsc msr pae mce cx8 apic sep mtrr pge mca cmov pat pse36 clflush mmx fxsr sse sse2 ss ht syscall nx pdpe1gb rdtscp lm constant_tsc rep_good nopl xtopology tsc_reliable nonstop_tsc cpuid aperfmperf tsc_known_freq pni pclmulqdq vmx ssse3 fma cx16 pcid sse4_1 sse4_2 x2apic movbe popcnt tsc_deadline_timer aes xsave avx f16c rdrand hypervisor lahf_lm abm 3dnowprefetch tpr_shadow ept vpid ept_ad fsgsbase tsc_adjust bmi1 hle avx2 smep bmi2 erms invpcid rtm avx512f avx512dq rdseed adx smap avx512ifma clflushopt clwb avx512cd sha_ni avx512bw avx512vl xsaveopt xsavec xgetbv1 xsaves user_shstk avx_vnni avx512_bf16 vnmi avx512vbmi umip waitpkg avx512_vbmi2 gfni vaes vpclmulqdq avx512_vnni avx512_bitalg avx512_vpopcntdq la57 rdpid cldemote movdiri movdir64b fsrm serialize tsxldtrk ibt amx_bf16 avx512_fp16 amx_tile amx_int8 arch_capabilities
Virtualization:                          VT-x
Hypervisor vendor:                       Microsoft
Virtualization type:                     full
L1d cache:                               96 KiB (2 instances)
L1i cache:                               64 KiB (2 instances)
L2 cache:                                4 MiB (2 instances)
L3 cache:                                260 MiB (1 instance)
NUMA node(s):                            1
NUMA node0 CPU(s):                       0-3
Vulnerability Gather data sampling:      Not affected
Vulnerability Ghostwrite:                Not affected
Vulnerability Indirect target selection: Mitigation; Aligned branch/return thunks
Vulnerability Itlb multihit:             Not affected
Vulnerability L1tf:                      Not affected
Vulnerability Mds:                       Not affected
Vulnerability Meltdown:                  Not affected
Vulnerability Mmio stale data:           Not affected
Vulnerability Old microcode:             Not affected
Vulnerability Reg file data sampling:    Not affected
Vulnerability Retbleed:                  Vulnerable
Vulnerability Spec rstack overflow:      Not affected
Vulnerability Spec store bypass:         Vulnerable
Vulnerability Spectre v1:                Mitigation; usercopy/swapgs barriers and __user pointer sanitization
Vulnerability Spectre v2:                Mitigation; Retpolines; STIBP disabled; RSB filling; PBRSB-eIBRS Not affected; BHI Retpoline
Vulnerability Srbds:                     Not affected
Vulnerability Tsa:                       Not affected
Vulnerability Tsx async abort:           Not affected
Vulnerability Vmscape:                   Not affected
```

</details>
