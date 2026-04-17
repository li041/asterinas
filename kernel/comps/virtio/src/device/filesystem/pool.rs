// SPDX-License-Identifier: MPL-2.0

//! DMA buffer pool for virtio-fs requests.
//!
//! This module provides [`FsDmaPool`], a size-class allocator backed by
//! [`DmaPool`] segments for small buffers and [`DmaStream`] for large ones.

use alloc::{collections::BTreeMap, sync::Arc};
use core::ops::Range;

use aster_network::dma_pool::{DmaPool, DmaSegment};
use aster_util::mem_obj_slice::Slice;
use ostd::{
    Result,
    mm::{
        HasDaddr, HasSize, Infallible, PAGE_SIZE, VmReader, VmWriter,
        dma::{DmaDirection, DmaStream},
        io::util::{HasVmReaderWriter, VmReaderWriterResult},
    },
    sync::SpinLock,
};

use crate::{device::VirtioDeviceError, dma_buf::DmaBuf};

const SIZE_CLASSES: &[usize] = &[64, 128, 256, 512, 1024, 2048, 4096];
const POOL_INIT_SIZE: usize = 8;
const POOL_HIGH_WATERMARK: usize = 64;

/// Represents a DMA-backed virtio-fs buffer slice.
pub type FsDmaBuf<D> = Slice<FsDmaStorage<D>>;

/// Represents a size-classed allocator for virtio-fs DMA buffers.
#[derive(Debug)]
pub struct FsDmaPool<D: DmaDirection> {
    pools: SpinLock<BTreeMap<usize, Arc<DmaPool<D>>>>,
}

impl<D: DmaDirection> FsDmaPool<D> {
    /// Creates a DMA buffer pool with predefined size classes.
    pub fn new() -> Arc<Self> {
        let mut pools = BTreeMap::new();
        for &class in SIZE_CLASSES {
            pools.insert(
                class,
                DmaPool::<D>::new(class, POOL_INIT_SIZE, POOL_HIGH_WATERMARK, false),
            );
        }

        Arc::new(Self {
            pools: SpinLock::new(pools),
        })
    }

    /// Allocates a DMA buffer whose visible length is `len`.
    pub fn alloc(&self, len: usize) -> core::result::Result<FsDmaBuf<D>, VirtioDeviceError> {
        let storage = if let Some(&class_size) = SIZE_CLASSES.iter().find(|&&size| size >= len) {
            let segment = {
                let pools = self.pools.disable_irq().lock();
                let pool = pools.get(&class_size).expect("size class always present");
                pool.alloc_segment()
                    .map_err(|_| VirtioDeviceError::ResourceAllocError)
            }?;

            FsDmaStorage::Segment(segment)
        } else {
            let stream = DmaStream::alloc_uninit(len.div_ceil(PAGE_SIZE), false)
                .map_err(|_| VirtioDeviceError::ResourceAllocError)?;
            FsDmaStorage::Stream(stream)
        };

        Ok(Slice::new(storage, 0..len))
    }
}

/// Represents the backing storage for a virtio-fs DMA buffer.
#[derive(Debug)]
pub enum FsDmaStorage<D: DmaDirection> {
    Stream(DmaStream<D>),
    Segment(DmaSegment<D>),
}

impl<D: DmaDirection> FsDmaStorage<D> {
    /// Synchronizes `byte_range` from the device into memory.
    pub fn sync_from_device(&self, byte_range: Range<usize>) -> Result<()> {
        match self {
            Self::Stream(stream) => stream.sync_from_device(byte_range),
            Self::Segment(segment) => segment.sync_from_device(byte_range),
        }
    }

    /// Synchronizes `byte_range` from memory to the device.
    pub fn sync_to_device(&self, byte_range: Range<usize>) -> Result<()> {
        match self {
            Self::Stream(stream) => stream.sync_to_device(byte_range),
            Self::Segment(segment) => segment.sync_to_device(byte_range),
        }
    }
}

impl<D: DmaDirection> HasSize for FsDmaStorage<D> {
    fn size(&self) -> usize {
        match self {
            Self::Stream(stream) => stream.size(),
            Self::Segment(segment) => segment.size(),
        }
    }
}

impl<D: DmaDirection> HasDaddr for FsDmaStorage<D> {
    fn daddr(&self) -> ostd::mm::Daddr {
        match self {
            Self::Stream(stream) => stream.daddr(),
            Self::Segment(segment) => segment.daddr(),
        }
    }
}

impl<D: DmaDirection> HasVmReaderWriter for FsDmaStorage<D> {
    type Types = VmReaderWriterResult;

    fn reader(&self) -> ostd::prelude::Result<VmReader<'_, Infallible>> {
        match self {
            Self::Stream(stream) => stream.reader(),
            Self::Segment(segment) => segment.reader(),
        }
    }

    fn writer(&self) -> ostd::prelude::Result<VmWriter<'_, Infallible>> {
        match self {
            Self::Stream(stream) => stream.writer(),
            Self::Segment(segment) => segment.writer(),
        }
    }
}

impl<D: DmaDirection> DmaBuf for Slice<FsDmaStorage<D>> {
    fn len(&self) -> usize {
        self.size()
    }
}
