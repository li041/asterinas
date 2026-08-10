#!/bin/bash

# SPDX-License-Identifier: MPL-2.0

set -euo pipefail

ASTERINAS_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
RESULT_ROOT=${POC_RESULT_ROOT:-${ASTERINAS_DIR}/benchmark_results/virtiofs-read-poc}
SHARED_DIR=${POC_SHARED_DIR:-${ASTERINAS_DIR}/test/initramfs/build/virtiofs-read-poc}
VIRTIOFSD_BIN=${VIRTIOFSD_BIN:-/usr/libexec/virtiofsd}
VIRTIOFS_SOCKET=${VIRTIOFS_SOCKET:-/tmp/vhostqemu/virtiofs-read-poc.sock}
POC_MEM=${POC_MEM:-8G}
POC_SMP=${POC_SMP:-1}

ALL_VARIANTS=(baseline cached_batch_256 cached_skip_copy direct_skip_copy)

if [ ! -x "$VIRTIOFSD_BIN" ]; then
    echo "virtiofsd not found at $VIRTIOFSD_BIN" >&2
    exit 2
fi
if ! command -v jq >/dev/null 2>&1; then
    echo "jq is required" >&2
    exit 2
fi

mkdir -p "$RESULT_ROOT" "$SHARED_DIR"

cleanup_virtiofsd() {
    local pid_file=${VIRTIOFS_SOCKET}.pid
    if [ ! -f "$pid_file" ]; then
        return
    fi

    local virtiofsd_pid
    virtiofsd_pid=$(<"$pid_file")
    if [ -n "$virtiofsd_pid" ] && [ -r "/proc/${virtiofsd_pid}/comm" ] && \
        [ "$(<"/proc/${virtiofsd_pid}/comm")" = virtiofsd ]; then
        kill "$virtiofsd_pid" 2>/dev/null || true
        wait "$virtiofsd_pid" 2>/dev/null || true
    fi
    rm -f "$pid_file" "$VIRTIOFS_SOCKET"
}
trap cleanup_virtiofsd EXIT

bandwidth_to_mb_per_sec() {
    local value=$1
    local unit=$2

    awk -v value="$value" -v unit="$unit" 'BEGIN {
        factor["B/s"] = 0.000001;
        factor["KB/s"] = 0.001;
        factor["MB/s"] = 1;
        factor["GB/s"] = 1000;
        factor["KiB/s"] = 1024 / 1000000;
        factor["MiB/s"] = 1048576 / 1000000;
        factor["GiB/s"] = 1073741824 / 1000000;
        printf "%.6f\n", value * factor[unit];
    }'
}

extract_bandwidth() {
    local output_file=$1
    local occurrence=$2
    local bandwidth

    bandwidth=$(sed -n 's/.*READ: bw=\([0-9.]*\)\([KMGT]*i*B\/s\).*/\1 \2/p' \
        "$output_file" | sed -n "${occurrence}p")
    if [ -z "$bandwidth" ]; then
        echo "failed to extract read bandwidth from benchmark output" >&2
        exit 1
    fi

    bandwidth_to_mb_per_sec $bandwidth
}

run_variant() {
    local variant=$1
    local kernel_args=$2
    local output_file
    output_file=$(mktemp)

    echo "=== ${variant} ==="
    make -C "$ASTERINAS_DIR" run_kernel \
        BENCHMARK=fio/seq_read_bw/virtiofs_poc \
        EXTRA_KCMD_ARGS="$kernel_args" \
        SMP="$POC_SMP" MEM="$POC_MEM" ENABLE_KVM=1 RELEASE_LTO=1 \
        NETDEV=tap VHOST=on VIRTIOFS=on \
        VIRTIOFS_SOCKET="$VIRTIOFS_SOCKET" \
        VIRTIOFS_SHARED_DIR="$SHARED_DIR" \
        VIRTIOFSD="$VIRTIOFSD_BIN" | tee "$output_file"

    local cached_bandwidth
    local direct_bandwidth
    cached_bandwidth=$(extract_bandwidth "$output_file" 1)
    direct_bandwidth=$(extract_bandwidth "$output_file" 2)
    rm -f "$output_file"

    jq -n \
        --argjson cached "$cached_bandwidth" \
        --argjson direct "$direct_bandwidth" \
        '[
            {
                name: "Cached read bandwidth on Asterinas",
                unit: "MB/s",
                value: $cached,
                extra: "cached"
            },
            {
                name: "Direct read bandwidth on Asterinas",
                unit: "MB/s",
                value: $direct,
                extra: "direct"
            }
        ]' > "${RESULT_ROOT}/${variant}.json"
}

run_named_variant() {
    case "$1" in
        baseline)
            run_variant baseline ""
            ;;
        cached_batch_256)
            run_variant cached_batch_256 "page_cache.poc_read_batch_pages=256"
            ;;
        cached_skip_copy)
            run_variant cached_skip_copy "page_cache.poc_skip_read_copy"
            ;;
        direct_skip_copy)
            run_variant direct_skip_copy "virtiofs.poc_skip_direct_read_copy"
            ;;
        *)
            echo "unknown variant: $1" >&2
            echo "valid variants: ${ALL_VARIANTS[*]}" >&2
            exit 2
            ;;
    esac
}

if [ "$#" -eq 0 ]; then
    set -- "${ALL_VARIANTS[@]}"
fi

for variant in "$@"; do
    run_named_variant "$variant"
done

echo "Collected bandwidth results under ${RESULT_ROOT}"
