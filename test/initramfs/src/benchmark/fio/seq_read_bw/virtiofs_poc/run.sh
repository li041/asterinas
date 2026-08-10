#!/bin/sh

# SPDX-License-Identifier: MPL-2.0

set -eu

VIRTIOFS_TAG=aster-virtiofs
FIO_MOUNT_POINT=/virtiofs
FIO_TEST_FILE=${FIO_MOUNT_POINT}/fio-test
FIO_RESULT_DIR=${FIO_MOUNT_POINT}/poc-results

mkdir -p "$FIO_MOUNT_POINT"
if ! mountpoint -q "$FIO_MOUNT_POINT"; then
    mount -t virtiofs "$VIRTIOFS_TAG" "$FIO_MOUNT_POINT"
fi
mkdir -p "$FIO_RESULT_DIR"

run_fio() {
    direct="$1"
    io_mode="$2"

    echo "*** Collecting FIO JSON for ${io_mode} read ***"
    /benchmark/bin/fio -rw=read "-filename=${FIO_TEST_FILE}" -name=seqread \
        -size=1G -bs=1M -ioengine=sync "-direct=${direct}" -numjobs=1 \
        -fsync_on_close=1 -time_based=1 -ramp_time=60 -runtime=100 \
        --output-format=json "--output=${FIO_RESULT_DIR}/${io_mode}.json"
}

run_fio 0 cached
run_fio 1 direct
sync
