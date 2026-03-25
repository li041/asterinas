// SPDX-License-Identifier: MPL-2.0

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
pub struct CryptoDmaPools {
    to_device_pools: SpinLock<BTreeMap<usize, Arc<DmaPool<ToDevice>>>>,
    from_device_pools: SpinLock<BTreeMap<usize, Arc<DmaPool<FromDevice>>>>,
}

impl CryptoDmaPools {
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

    pub fn alloc_to_device(self: &Arc<Self>, required_len: usize) -> CryptoDmaBuf {
        let class = SIZE_CLASSES.iter().find(|&&size| size >= required_len);

        let storage = if let Some(class_size) = class {
            let pool = self
                .to_device_pools
                .disable_irq()
                .lock()
                .get(&class_size)
                .unwrap()
                .clone();
            CryptoDmaStorage::ToSegment(pool.alloc_segment().unwrap())
        } else {
            CryptoDmaStorage::Stream(Arc::new(
                DmaStream::alloc(required_len.div_ceil(PAGE_SIZE), false).unwrap(),
            ))
        };

        CryptoDmaBuf {
            storage: Arc::new(storage),
            required_len,
        }
    }

    pub fn alloc_from_device(self: &Arc<Self>, required_len: usize) -> CryptoDmaBuf {
        let class = SIZE_CLASSES.iter().find(|&&size| size >= required_len);

        let storage = if let Some(class_size) = class {
            let pool = self
                .from_device_pools
                .disable_irq()
                .lock()
                .get(&class_size)
                .unwrap()
                .clone();
            CryptoDmaStorage::FromSegment(pool.alloc_segment().unwrap())
        } else {
            CryptoDmaStorage::Stream(Arc::new(
                DmaStream::alloc(required_len.div_ceil(PAGE_SIZE), false).unwrap(),
            ))
        };

        CryptoDmaBuf {
            storage: Arc::new(storage),
            required_len,
        }
    }
}

#[derive(Debug)]
enum CryptoDmaStorage {
    ToSegment(DmaSegment<ToDevice>),
    FromSegment(DmaSegment<FromDevice>),
    Stream(Arc<DmaStream>),
}

#[derive(Debug, Clone)]
pub struct CryptoDmaBuf {
    storage: Arc<CryptoDmaStorage>,
    required_len: usize,
}

impl CryptoDmaBuf {
    pub fn sync_from_device(&self, byte_range: Range<usize>) -> Result<()> {
        match self.storage.as_ref() {
            CryptoDmaStorage::ToSegment(_) => Ok(()),
            CryptoDmaStorage::FromSegment(segment) => segment.sync_from_device(byte_range),
            CryptoDmaStorage::Stream(stream) => stream.sync_from_device(byte_range),
        }
    }

    pub fn sync_to_device(&self, byte_range: Range<usize>) -> Result<()> {
        match self.storage.as_ref() {
            CryptoDmaStorage::ToSegment(segment) => segment.sync_to_device(byte_range),
            CryptoDmaStorage::FromSegment(_) => Ok(()),
            CryptoDmaStorage::Stream(stream) => stream.sync_to_device(byte_range),
        }
    }
}

impl HasSize for CryptoDmaBuf {
    fn size(&self) -> usize {
        self.required_len
    }
}

impl HasDaddr for CryptoDmaBuf {
    fn daddr(&self) -> ostd::mm::Daddr {
        match self.storage.as_ref() {
            CryptoDmaStorage::ToSegment(segment) => segment.daddr(),
            CryptoDmaStorage::FromSegment(segment) => segment.daddr(),
            CryptoDmaStorage::Stream(stream) => stream.daddr(),
        }
    }
}

impl HasVmReaderWriter for CryptoDmaBuf {
    type Types = VmReaderWriterResult;

    fn reader(&self) -> ostd::prelude::Result<VmReader<'_, Infallible>> {
        let mut reader = match self.storage.as_ref() {
            CryptoDmaStorage::ToSegment(segment) => segment.reader()?,
            CryptoDmaStorage::FromSegment(segment) => segment.reader()?,
            CryptoDmaStorage::Stream(stream) => stream.reader()?,
        };
        reader.limit(self.required_len);
        Ok(reader)
    }

    fn writer(&self) -> ostd::prelude::Result<VmWriter<'_, Infallible>> {
        let mut writer = match self.storage.as_ref() {
            CryptoDmaStorage::ToSegment(segment) => segment.writer()?,
            CryptoDmaStorage::FromSegment(segment) => segment.writer()?,
            CryptoDmaStorage::Stream(stream) => stream.writer()?,
        };
        writer.limit(self.required_len);
        Ok(writer)
    }
}

impl DmaBuf for CryptoDmaBuf {
    fn len(&self) -> usize {
        self.size()
    }
}

impl DmaBuf for Slice<CryptoDmaBuf> {
    fn len(&self) -> usize {
        self.size()
    }
}
