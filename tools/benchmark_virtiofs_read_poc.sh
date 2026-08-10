#!/bin/bash

# SPDX-License-Identifier: MPL-2.0

set -euo pipefail

ASTERINAS_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
RESULT_ROOT=${POC_RESULT_ROOT:-${ASTERINAS_DIR}/benchmark_results/virtiofs-read-poc}
SHARED_DIR=${POC_SHARED_DIR:-${ASTERINAS_DIR}/test/initramfs/build/virtiofs-read-poc}
VIRTIOFSD_BIN=${VIRTIOFSD_BIN:-/usr/libexec/virtiofsd}
VIRTIOFS_SOCKET=${VIRTIOFS_SOCKET:-/tmp/vhostqemu/virtiofs-read-poc.sock}
POC_REPEATS=${POC_REPEATS:-1}
POC_MEM=${POC_MEM:-8G}
POC_SMP=${POC_SMP:-1}

if ! [[ "$POC_REPEATS" =~ ^[1-9][0-9]*$ ]]; then
    echo "POC_REPEATS must be a positive integer" >&2
    exit 2
fi
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

run_variant() {
    local variant=$1
    local kernel_args=$2
    local run_index=$3
    local run_dir=${RESULT_ROOT}/${variant}/run-${run_index}
    local guest_result_dir=${SHARED_DIR}/poc-results

    mkdir -p "$run_dir" "$guest_result_dir"
    rm -f "${guest_result_dir}/cached.json" "${guest_result_dir}/direct.json"

    echo "=== ${variant}, run ${run_index}/${POC_REPEATS} ==="
    make -C "$ASTERINAS_DIR" run_kernel \
        BENCHMARK=fio/seq_read_bw/virtiofs_poc \
        EXTRA_KCMD_ARGS="$kernel_args" \
        SMP="$POC_SMP" MEM="$POC_MEM" ENABLE_KVM=1 RELEASE_LTO=1 \
        NETDEV=tap VHOST=on VIRTIOFS=on \
        VIRTIOFS_SOCKET="$VIRTIOFS_SOCKET" \
        VIRTIOFS_SHARED_DIR="$SHARED_DIR" \
        VIRTIOFSD="$VIRTIOFSD_BIN"

    for mode in cached direct; do
        local source_json=${guest_result_dir}/${mode}.json
        if [ ! -f "$source_json" ]; then
            echo "missing fio result: $source_json" >&2
            exit 1
        fi
        jq empty "$source_json"
        cp "$source_json" "${run_dir}/${mode}.json"
    done

    jq -n \
        --arg variant "$variant" \
        --arg kernel_args "$kernel_args" \
        --arg commit "$(git -C "$ASTERINAS_DIR" rev-parse HEAD)" \
        --arg collected_at "$(date --utc +%Y-%m-%dT%H:%M:%SZ)" \
        --arg memory "$POC_MEM" \
        --argjson smp "$POC_SMP" \
        --arg virtiofsd_version "$($VIRTIOFSD_BIN --version 2>&1 | head -n 1)" \
        --slurpfile cached "${run_dir}/cached.json" \
        --slurpfile direct "${run_dir}/direct.json" \
        '{
            variant: $variant,
            kernel_args: $kernel_args,
            commit: $commit,
            collected_at: $collected_at,
            memory: $memory,
            smp: $smp,
            virtiofsd: $virtiofsd_version,
            cached: {
                bandwidth_bytes_per_sec: $cached[0].jobs[0].read.bw_bytes,
                iops: $cached[0].jobs[0].read.iops,
                runtime_ms: $cached[0].jobs[0].read.runtime,
                mean_completion_latency_ns: $cached[0].jobs[0].read.clat_ns.mean
            },
            direct: {
                bandwidth_bytes_per_sec: $direct[0].jobs[0].read.bw_bytes,
                iops: $direct[0].jobs[0].read.iops,
                runtime_ms: $direct[0].jobs[0].read.runtime,
                mean_completion_latency_ns: $direct[0].jobs[0].read.clat_ns.mean
            }
        }' > "${run_dir}/summary.json"
}

for run_index in $(seq 1 "$POC_REPEATS"); do
    run_variant baseline "" "$run_index"
    run_variant cached_batch_256 "page_cache.poc_read_batch_pages=256" "$run_index"
    run_variant cached_skip_copy "page_cache.poc_skip_read_copy" "$run_index"
    run_variant direct_skip_copy "virtiofs.poc_skip_direct_read_copy" "$run_index"
done

find "$RESULT_ROOT" -mindepth 3 -maxdepth 3 -name summary.json -print0 | \
    sort -z | xargs -0 jq -s \
        --arg commit "$(git -C "$ASTERINAS_DIR" rev-parse HEAD)" \
        '{commit: $commit, experiments: .}' > "${RESULT_ROOT}/summary.json"

echo "Collected fio JSON under ${RESULT_ROOT}"
