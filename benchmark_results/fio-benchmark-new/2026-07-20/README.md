# fio-benchmark-new results

Source commit: `897734c74304541ad1bc97e2b070b619e962e223`

Each benchmark was run three times. Values are the benchmark script's MB/s output; raw JSON strings are preserved unchanged. Averages are calculated from those strings without pre-rounding and displayed to six decimal places.

| Filesystem | Workload | Mode | System | Run 1 (MB/s) | Run 2 (MB/s) | Run 3 (MB/s) | Average (MB/s) |
|---|---|---|---|---:|---:|---:|---:|
| Ext2 | read | cached | Linux | 35218.7 | 34789.2 | 35433.5 | 35147.133333 |
| Ext2 | read | cached | Asterinas | 19756.8 | 19971.6 | 19864.2 | 19864.200000 |
| Ext2 | read | direct | Linux | 9668.92 | 7295.99 | 8565.82 | 8510.243333 |
| Ext2 | read | direct | Asterinas | 6169.82 | 5889.85 | 5301.6 | 5787.090000 |
| Ext2 | write | cached | Linux | 8366.59 | 10326.4 | 7959.74 | 8884.243333 |
| Ext2 | write | cached | Asterinas | 18253.6 | 18253.6 | 18361 | 18289.400000 |
| Ext2 | write | direct | Linux | 2178.94 | 1551.89 | 2084.57 | 1938.466667 |
| Ext2 | write | direct | Asterinas | 3490.71 | 1443.89 | 3476.03 | 2803.543333 |
| virtio-fs | read | cached | Linux | 33822.9 | 33071.2 | 33930.2 | 33608.100000 |
| virtio-fs | read | cached | Asterinas | 18897.9 | 19327.4 | 19434.7 | 19220.000000 |
| virtio-fs | read | direct | Linux | 13314.4 | 17824.1 | 13958.6 | 15032.366667 |
| virtio-fs | read | direct | Asterinas | 8014.27 | 11166.9 | 9439.28 | 9540.150000 |
| virtio-fs | write | cached | Linux | 3294.63 | 4225.76 | 4220.52 | 3913.636667 |
| virtio-fs | write | cached | Asterinas | 719.323 | 631.243 | 714.08 | 688.215333 |
| virtio-fs | write | direct | Linux | 1888.49 | 2080.37 | 2347.76 | 2105.540000 |
| virtio-fs | write | direct | Asterinas | 5916.07 | 4322.23 | 5714.74 | 5317.680000 |
