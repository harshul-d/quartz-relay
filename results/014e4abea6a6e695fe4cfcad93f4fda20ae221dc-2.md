# Benchmark 014e4abea6a6e695fe4cfcad93f4fda20ae221dc

- CPU: AMD EPYC 7763 64-Core Processor; 4 logical CPUs; x86-64-v3 flags all present; AVX-512: no
- Data: `big`, 1.89 GB; 5 repetitions; docker flags of scripts/docker_run.sh with --cpus 4 --memory 24g
- Transparent huge pages: runner default `madvise`, defrag `madvise`
- Variants: `v9` = v9; `head-late` = HEAD PIPELINE_EARLY_WRITE=0; `head` = HEAD

## Wall time, seconds: median (min-max)

| Run | cold | warm |
| --- | --- | --- |
| v9 | 5.166 (5.139-5.178) | 1.992 (1.982-2.009) |
| head-late | 5.143 (5.130-5.178) | 1.983 (1.980-2.057) |
| head | 5.141 (5.122-5.174) | 1.987 (1.962-1.990) |
| v9 /bin/true | 0.373 (0.356-0.395) | 0.159 (0.153-0.181) |
| head-late /bin/true | 0.376 (0.370-0.383) | 0.162 (0.154-0.171) |
| head /bin/true | 0.372 (0.360-0.389) | 0.155 (0.154-0.159) |
| v9 native (warm) | - | 1.849 (1.820-1.867) |
| head-late native (warm) | - | 1.846 (1.824-1.860) |
| head native (warm) | - | 1.867 (1.823-1.882) |
| cat of all input, 4 readers | 4.345 (4.326-4.353) | 0.044 (0.044-0.046) |
| cat of all input, 16 readers | 4.466 (4.453-4.509) | 0.047 (0.045-0.047) |

## Phases of `v9` (seconds, median over runs)

| Phase | container cold | container warm | native warm |
| --- | --- | --- | --- |
| lookups | 0.017 | 0.012 | 0.013 |
| ID linking | 0.114 | 0.099 | 0.105 |
| events read+parse | 3.802 | 1.005 | 0.997 |
| dedup | 0.077 | 0.075 | 0.075 |
| clicks read+parse | 0.290 | 0.161 | 0.160 |
| sessionize+attribution | 0.413 | 0.416 | 0.415 |
| output writing | 0.050 | 0.051 | 0.051 |
| report+drops | 0.000 | 0.000 | 0.000 |
| outside main (start, exit, container) | 0.388 | 0.173 | 0.020 |
| start: run start -> main | 0.329 | 0.121 | 0.010 |
| exit: end of main -> run end | 0.059 | 0.053 | 0.010 |
| cpu-s read (all threads) | 10.429 | 0.176 | 0.147 |
| cpu-s group-events (all threads) | 0.708 | 0.717 | 0.713 |
| cpu-s group-clicks (all threads) | 0.294 | 0.292 | 0.294 |
| cpu-s sessions (all threads) | 0.190 | 0.190 | 0.190 |
| cpu-s attribution (all threads) | 0.452 | 0.450 | 0.454 |

Native `/usr/bin/time -v` of `v9` (median):

| max RSS MiB | minor faults | major faults | voluntary switches | involuntary switches | user s | system s |
| --- | --- | --- | --- | --- | --- | --- |
| 2047.79 | 2281.00 | 25.00 | 69.00 | 1127.00 | 6.56 | 0.48 |

## Phases of `head-late` (seconds, median over runs)

| Phase | container cold | container warm | native warm |
| --- | --- | --- | --- |
| lookups | 0.016 | 0.012 | 0.014 |
| ID linking | 0.105 | 0.097 | 0.101 |
| events read+parse | 3.799 | 1.005 | 0.996 |
| dedup | 0.077 | 0.076 | 0.074 |
| clicks read+parse | 0.290 | 0.159 | 0.159 |
| sessionize+attribution | 0.408 | 0.411 | 0.408 |
| output writing | 0.053 | 0.052 | 0.052 |
| report+drops | 0.000 | 0.000 | 0.000 |
| outside main (start, exit, container) | 0.393 | 0.177 | 0.040 |
| start: run start -> main | 0.330 | 0.121 | 0.030 |
| exit: end of main -> run end | 0.062 | 0.055 | 0.010 |
| cpu-s read (all threads) | 10.430 | 0.177 | 0.145 |
| cpu-s group-events (all threads) | 0.699 | 0.710 | 0.703 |
| cpu-s group-clicks (all threads) | 0.290 | 0.290 | 0.290 |
| cpu-s sessions (all threads) | 0.173 | 0.171 | 0.172 |
| cpu-s attribution (all threads) | 0.458 | 0.460 | 0.458 |

Native `/usr/bin/time -v` of `head-late` (median):

| max RSS MiB | minor faults | major faults | voluntary switches | involuntary switches | user s | system s |
| --- | --- | --- | --- | --- | --- | --- |
| 2049.63 | 2295.00 | 27.00 | 61.00 | 978.00 | 6.55 | 0.48 |

## Phases of `head` (seconds, median over runs)

| Phase | container cold | container warm | native warm |
| --- | --- | --- | --- |
| lookups | 0.017 | 0.011 | 0.013 |
| ID linking | 0.113 | 0.106 | 0.102 |
| events read+parse | 3.802 | 1.002 | 1.003 |
| dedup | 0.076 | 0.074 | 0.073 |
| clicks read+parse | 0.292 | 0.159 | 0.160 |
| sessionize+attribution | 0.456 | 0.456 | 0.466 |
| output writing | 0.004 | 0.004 | 0.004 |
| report+drops | 0.000 | 0.000 | 0.000 |
| outside main (start, exit, container) | 0.379 | 0.172 | 0.018 |
| start: run start -> main | 0.323 | 0.121 | 0.010 |
| exit: end of main -> run end | 0.053 | 0.051 | 0.009 |
| cpu-s read (all threads) | 10.374 | 0.176 | 0.147 |
| cpu-s group-events (all threads) | 0.709 | 0.705 | 0.723 |
| cpu-s group-clicks (all threads) | 0.295 | 0.294 | 0.302 |
| cpu-s sessions (all threads) | 0.156 | 0.157 | 0.156 |
| cpu-s attribution (all threads) | 0.459 | 0.457 | 0.467 |

Native `/usr/bin/time -v` of `head` (median):

| max RSS MiB | minor faults | major faults | voluntary switches | involuntary switches | user s | system s |
| --- | --- | --- | --- | --- | --- | --- |
| 2035.84 | 2268.00 | 27.00 | 68.00 | 1214.00 | 6.56 | 0.48 |

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
Vendor ID:                               AuthenticAMD
Model name:                              AMD EPYC 7763 64-Core Processor
CPU family:                              25
Model:                                   1
Thread(s) per core:                      2
Core(s) per socket:                      2
Socket(s):                               1
Stepping:                                1
BogoMIPS:                                4890.86
Flags:                                   fpu vme de pse tsc msr pae mce cx8 apic sep mtrr pge mca cmov pat pse36 clflush mmx fxsr sse sse2 ht syscall nx mmxext fxsr_opt pdpe1gb rdtscp lm constant_tsc rep_good nopl tsc_reliable nonstop_tsc cpuid extd_apicid aperfmperf tsc_known_freq pni pclmulqdq ssse3 fma cx16 pcid sse4_1 sse4_2 movbe popcnt aes xsave avx f16c rdrand hypervisor lahf_lm cmp_legacy svm cr8_legacy abm sse4a misalignsse 3dnowprefetch osvw topoext vmmcall fsgsbase bmi1 avx2 smep bmi2 erms invpcid rdseed adx smap clflushopt clwb sha_ni xsaveopt xsavec xgetbv1 xsaves user_shstk clzero xsaveerptr rdpru arat npt nrip_save tsc_scale vmcb_clean flushbyasid decodeassists pausefilter pfthreshold v_vmsave_vmload umip vaes vpclmulqdq rdpid fsrm
Virtualization:                          AMD-V
Hypervisor vendor:                       Microsoft
Virtualization type:                     full
L1d cache:                               64 KiB (2 instances)
L1i cache:                               64 KiB (2 instances)
L2 cache:                                1 MiB (2 instances)
L3 cache:                                32 MiB (1 instance)
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
Vulnerability Spec rstack overflow:      Vulnerable: Safe RET, no microcode
Vulnerability Spec store bypass:         Vulnerable
Vulnerability Spectre v1:                Mitigation; usercopy/swapgs barriers and __user pointer sanitization
Vulnerability Spectre v2:                Mitigation; Retpolines; STIBP disabled; RSB filling; PBRSB-eIBRS Not affected; BHI Not affected
Vulnerability Srbds:                     Not affected
Vulnerability Tsa:                       Vulnerable: No microcode
Vulnerability Tsx async abort:           Not affected
Vulnerability Vmscape:                   Not affected
```

</details>
