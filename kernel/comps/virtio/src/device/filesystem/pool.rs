// SPDX-License-Identifier: MPL-2.0

//! DMA buffer pool for virtio-fs requests.
//!
//! This module provides [`FsDmaPool`], a size-class allocator backed by
//! [`DmaPool`] segments for small buffers and [`DmaStream`] for large ones.

use alloc::sync::Arc;
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
};

use crate::dma_buf::DmaBuf;

/// Pool-backed buffers start at 64 bytes, enough for small fixed-size FUSE
/// request and reply payloads without wasting a page per request.
const MIN_SHIFT: u32 = 6;

/// Pool-backed buffers stop at one page. Larger buffers use `DmaStream` so large
/// reads and writes do not consume all small-buffer pool segments.
const MAX_SHIFT: u32 = 12;
const N_CLASSES: usize = (MAX_SHIFT - MIN_SHIFT + 1) as usize;
const MAX_CLASS_SIZE: usize = 1 << MAX_SHIFT;

/// Preallocate a few segments per size class to keep the common request path from
/// allocating under light concurrency.
const POOL_INIT_SIZE: usize = 8;

/// Retain enough free segments for bursts
const POOL_HIGH_WATERMARK: usize = 64;

/// A DMA-backed virtio-fs buffer slice.
pub type FsDmaBuf<D> = Slice<FsDmaStorage<D>>;

/// A size-classed allocator for virtio-fs DMA buffers.
#[derive(Debug)]
pub struct FsDmaPool<D: DmaDirection> {
    classes: [Arc<DmaPool<D>>; N_CLASSES],
}

impl<D: DmaDirection> FsDmaPool<D> {
    /// Creates a DMA buffer pool with predefined size classes.
    pub fn new() -> Arc<Self> {
        let classes = core::array::from_fn(|i| {
            let segment_size = 1usize << (MIN_SHIFT + i as u32);
            DmaPool::<D>::new(segment_size, POOL_INIT_SIZE, POOL_HIGH_WATERMARK, false)
        });
        Arc::new(Self { classes })
    }

    /// Allocates a DMA buffer whose visible length is `len`.
    pub fn alloc_fs_buf(&self, len: usize) -> Result<FsDmaBuf<D>> {
        let storage = if len <= MAX_CLASS_SIZE {
            let shift = len.next_power_of_two().trailing_zeros().max(MIN_SHIFT);
            let segment = self.classes[(shift - MIN_SHIFT) as usize].alloc_segment()?;
            FsDmaStorage::Segment(segment)
        } else {
            let stream = DmaStream::alloc_uninit(len.div_ceil(PAGE_SIZE), false)?;
            FsDmaStorage::Stream(stream)
        };

        Ok(Slice::new(storage, 0..len))
    }
}

/// The backing storage for a virtio-fs DMA buffer.
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

    fn reader(&self) -> Result<VmReader<'_, Infallible>> {
        match self {
            Self::Stream(stream) => stream.reader(),
            Self::Segment(segment) => segment.reader(),
        }
    }

    fn writer(&self) -> Result<VmWriter<'_, Infallible>> {
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
