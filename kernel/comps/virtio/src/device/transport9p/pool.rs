// SPDX-License-Identifier: MPL-2.0

//! DMA buffer pool for 9P transport — adapted from filesystem/pool.rs.

use alloc::{collections::BTreeMap, sync::Arc};
use core::ops::Range;

use aster_network::{DmaSegment, dma_pool::DmaPool};
use aster_util::mem_obj_slice::Slice;
use ostd::{
    Result,
    mm::{
        HasDaddr, HasSize, Infallible, PAGE_SIZE, VmReader, VmWriter,
        dma::{DmaStream, FromDevice, ToDevice},
        io_util::{HasVmReaderWriter, VmReaderWriterResult},
    },
    sync::SpinLock,
};

use crate::dma_buf::DmaBuf;

const SIZE_CLASSES: &[usize] = &[64, 128, 256, 512, 1024, 2048, 4096];
const POOL_INIT_SIZE: usize = 8;
const POOL_HIGH_WATERMARK: usize = 64;

#[derive(Debug)]
pub struct P9DmaPools {
    to_device_pools: SpinLock<BTreeMap<usize, Arc<DmaPool<ToDevice>>>>,
    from_device_pools: SpinLock<BTreeMap<usize, Arc<DmaPool<FromDevice>>>>,
}

impl P9DmaPools {
    pub fn new() -> Arc<Self> {
        let mut to_device_pools = BTreeMap::new();
        let mut from_device_pools = BTreeMap::new();
        for &class in SIZE_CLASSES {
            to_device_pools.insert(
                class,
                DmaPool::<ToDevice>::new(class, POOL_INIT_SIZE, POOL_HIGH_WATERMARK, false),
            );
            from_device_pools.insert(
                class,
                DmaPool::<FromDevice>::new(class, POOL_INIT_SIZE, POOL_HIGH_WATERMARK, false),
            );
        }

        Arc::new(Self {
            to_device_pools: SpinLock::new(to_device_pools),
            from_device_pools: SpinLock::new(from_device_pools),
        })
    }

    pub fn alloc_to_device(self: &Arc<Self>, required_len: usize) -> P9DmaBuf {
        let class = SIZE_CLASSES.iter().find(|&&size| size >= required_len);

        let storage = if let Some(class_size) = class {
            let pool = self
                .to_device_pools
                .disable_irq()
                .lock()
                .get(&class_size)
                .unwrap()
                .clone();
            P9DmaStorage::ToSegment(pool.alloc_segment().unwrap())
        } else {
            P9DmaStorage::Stream(Arc::new(
                DmaStream::alloc(required_len.div_ceil(PAGE_SIZE), false).unwrap(),
            ))
        };

        P9DmaBuf {
            storage: Arc::new(storage),
            required_len,
        }
    }

    pub fn alloc_from_device(self: &Arc<Self>, required_len: usize) -> P9DmaBuf {
        let class = SIZE_CLASSES.iter().find(|&&size| size >= required_len);

        let storage = if let Some(class_size) = class {
            let pool = self
                .from_device_pools
                .disable_irq()
                .lock()
                .get(&class_size)
                .unwrap()
                .clone();
            P9DmaStorage::FromSegment(pool.alloc_segment().unwrap())
        } else {
            P9DmaStorage::Stream(Arc::new(
                DmaStream::alloc(required_len.div_ceil(PAGE_SIZE), false).unwrap(),
            ))
        };

        P9DmaBuf {
            storage: Arc::new(storage),
            required_len,
        }
    }
}

#[derive(Debug)]
enum P9DmaStorage {
    ToSegment(DmaSegment<ToDevice>),
    FromSegment(DmaSegment<FromDevice>),
    Stream(Arc<DmaStream>),
}

#[derive(Debug, Clone)]
pub struct P9DmaBuf {
    storage: Arc<P9DmaStorage>,
    required_len: usize,
}

impl P9DmaBuf {
    pub fn sync_from_device(&self, byte_range: Range<usize>) -> Result<()> {
        match self.storage.as_ref() {
            P9DmaStorage::ToSegment(_) => Ok(()),
            P9DmaStorage::FromSegment(segment) => segment.sync_from_device(byte_range),
            P9DmaStorage::Stream(stream) => stream.sync_from_device(byte_range),
        }
    }

    pub fn sync_to_device(&self, byte_range: Range<usize>) -> Result<()> {
        match self.storage.as_ref() {
            P9DmaStorage::ToSegment(segment) => segment.sync_to_device(byte_range),
            P9DmaStorage::FromSegment(_) => Ok(()),
            P9DmaStorage::Stream(stream) => stream.sync_to_device(byte_range),
        }
    }
}

impl HasSize for P9DmaBuf {
    fn size(&self) -> usize {
        self.required_len
    }
}

impl HasDaddr for P9DmaBuf {
    fn daddr(&self) -> ostd::mm::Daddr {
        match self.storage.as_ref() {
            P9DmaStorage::ToSegment(segment) => segment.daddr(),
            P9DmaStorage::FromSegment(segment) => segment.daddr(),
            P9DmaStorage::Stream(stream) => stream.daddr(),
        }
    }
}

impl HasVmReaderWriter for P9DmaBuf {
    type Types = VmReaderWriterResult;

    fn reader(&self) -> ostd::prelude::Result<VmReader<'_, Infallible>> {
        let mut reader = match self.storage.as_ref() {
            P9DmaStorage::ToSegment(segment) => segment.reader()?,
            P9DmaStorage::FromSegment(segment) => segment.reader()?,
            P9DmaStorage::Stream(stream) => stream.reader()?,
        };
        reader.limit(self.required_len);
        Ok(reader)
    }

    fn writer(&self) -> ostd::prelude::Result<VmWriter<'_, Infallible>> {
        let mut writer = match self.storage.as_ref() {
            P9DmaStorage::ToSegment(segment) => segment.writer()?,
            P9DmaStorage::FromSegment(segment) => segment.writer()?,
            P9DmaStorage::Stream(stream) => stream.writer()?,
        };
        writer.limit(self.required_len);
        Ok(writer)
    }
}

impl DmaBuf for P9DmaBuf {
    fn len(&self) -> usize {
        self.size()
    }
}

impl DmaBuf for Slice<P9DmaBuf> {
    fn len(&self) -> usize {
        self.size()
    }
}
