// SPDX-License-Identifier: MPL-2.0

use alloc::{collections::BTreeMap, sync::Arc};

use aster_network::dma_pool::{DmaPool, DmaSegment};
use aster_util::mem_obj_slice::Slice;
use ostd::{
    mm::{
        HasSize, PAGE_SIZE,
        dma::{DmaDirection, DmaStream, FromDevice, ToDevice},
    },
    sync::SpinLock,
};

use crate::{device::VirtioDeviceError, dma_buf::DmaBuf};

const SIZE_CLASSES: &[usize] = &[64, 128, 256, 512, 1024, 2048, 4096];
const POOL_INIT_SIZE: usize = 8;
const POOL_HIGH_WATERMARK: usize = 64;

pub type ToDeviceDmaSegmentSlice = Slice<Arc<DmaSegment<ToDevice>>>;
pub type ToDeviceDmaStreamSlice = Slice<Arc<DmaStream<ToDevice>>>;
pub type FromDeviceDmaSegmentSlice = Slice<Arc<DmaSegment<FromDevice>>>;
pub type FromDeviceDmaStreamSlice = Slice<Arc<DmaStream<FromDevice>>>;
pub type FsRequestDmaBuf = Arc<dyn DmaBuf + Send + Sync>;

#[derive(Debug)]
pub struct FsDmaPool<D: DmaDirection>(SpinLock<BTreeMap<usize, Arc<DmaPool<D>>>>);

impl<D: DmaDirection> FsDmaPool<D> {
    pub fn new() -> Self {
        let mut pools = BTreeMap::new();
        for &class_size in SIZE_CLASSES {
            pools.insert(
                class_size,
                DmaPool::<D>::new(class_size, POOL_INIT_SIZE, POOL_HIGH_WATERMARK, false),
            );
        }

        Self(SpinLock::new(pools))
    }

    pub fn alloc_segment(
        &self,
        required_len: usize,
    ) -> core::result::Result<Arc<DmaSegment<D>>, VirtioDeviceError> {
        if required_len > PAGE_SIZE {
            return Err(VirtioDeviceError::QueueUnknownError);
        }

        let pools = self.0.disable_irq().lock();
        let pool = pools
            .range(required_len..)
            .next()
            .map(|(_, pool)| pool)
            .ok_or(VirtioDeviceError::QueueUnknownError)?;
        let segment = pool
            .alloc_segment()
            .map_err(|_| VirtioDeviceError::QueueUnknownError)?;
        Ok(Arc::new(segment))
    }
}

impl FsDmaPool<ToDevice> {
    pub fn alloc_stream(
        &self,
        required_len: usize,
    ) -> core::result::Result<Arc<DmaStream<ToDevice>>, VirtioDeviceError> {
        let stream = DmaStream::alloc(required_len.div_ceil(PAGE_SIZE), false)
            .map_err(|_| VirtioDeviceError::QueueUnknownError)?;
        Ok(Arc::new(stream))
    }
}

impl FsDmaPool<FromDevice> {
    pub fn alloc_stream(
        &self,
        required_len: usize,
    ) -> core::result::Result<Arc<DmaStream<FromDevice>>, VirtioDeviceError> {
        let stream = DmaStream::alloc_uninit(required_len.div_ceil(PAGE_SIZE), false)
            .map_err(|_| VirtioDeviceError::QueueUnknownError)?;
        Ok(Arc::new(stream))
    }
}

pub fn as_dma_buf(dma_buf: &impl DmaBuf) -> &dyn DmaBuf {
    dma_buf
}

pub fn request_dma_buf<T>(dma_slice: &Slice<Arc<T>>) -> FsRequestDmaBuf
where
    T: DmaBuf + HasSize + Send + Sync + 'static,
{
    dma_slice.mem_obj().clone()
}
