// SPDX-License-Identifier: MPL-2.0

use alloc::{boxed::Box, format, sync::Arc};
use core::sync::atomic::{AtomicUsize, Ordering};

use ostd::{
    Error,
    arch::trap::TrapFrame,
    mm::{
        Fallible, FallibleVmRead, FrameAllocOptions, HasSize, PAGE_SIZE, VmWriter,
        dma::{DmaStream, FromDevice},
        io::util::HasVmReaderWriter,
    },
    sync::{SpinLock, WaitQueue},
};

use crate::{
    device::{VirtioDeviceError, entropy::register_device},
    queue::VirtQueue,
    transport::VirtioTransport,
};

const ENTROPY_DEVICE_PREFIX: &str = "virtio_rng.";
static ENTROPY_DEVICE_ID: AtomicUsize = AtomicUsize::new(0);

const ENTROPY_QUEUE_SIZE: u16 = 1;
const ENTROPY_BUFFER_SIZE: usize = PAGE_SIZE;

/// Entropy devices, which supply high-quality randomness for guest use.
pub struct EntropyDevice {
    transport: SpinLock<Box<dyn VirtioTransport>>,
    inner: SpinLock<EntropyDeviceInner>,
    wait_queue: WaitQueue,
}

impl EntropyDevice {
    pub(crate) fn init(mut transport: Box<dyn VirtioTransport>) -> Result<(), VirtioDeviceError> {
        let inner = SpinLock::new(EntropyDeviceInner::new(VirtQueue::new(
            0,
            ENTROPY_QUEUE_SIZE,
            transport.as_mut(),
        )?));
        let device = Arc::new(EntropyDevice {
            transport: SpinLock::new(transport),
            inner,
            wait_queue: WaitQueue::new(),
        });

        // Register IRQ callbacks.
        let mut transport = device.transport.lock();

        // Virtio-rng has no configuration fields, so config-space change interrupts
        // are not expected and no config callback is registered.
        transport.register_queue_callback(
            0,
            Box::new({
                let device = device.clone();
                move |_: &TrapFrame| device.handle_recv_irq()
            }),
            false,
        )?;

        transport.finish_init();
        drop(transport);

        let device_id = ENTROPY_DEVICE_ID.fetch_add(1, Ordering::Relaxed);
        let name = format!("{ENTROPY_DEVICE_PREFIX}{device_id}");

        register_device(name, device);

        Ok(())
    }

    /// Copies up to `writer.avail()` bytes into `writer`.
    /// Returns `Ok(n)` with `n > 0` on success, `Ok(0)` iff `writer.avail() == 0`,
    /// `Ok(None)` if no cached bytes are available (caller decides whether to block).
    pub fn try_read_into(
        &self,
        writer: &mut VmWriter<'_, Fallible>,
    ) -> core::result::Result<Option<usize>, Error> {
        let mut inner = self.inner.lock();

        // 1) Drain any cached lefovers first - no vq interaction.
        if inner.data_idx < inner.data_avail {
            let available = inner.data_avail - inner.data_idx;
            let read_len = available.min(writer.avail());

            let copied = {
                let mut reader = inner.buffer.reader()?;
                reader
                    .skip(inner.data_idx)
                    .limit(read_len)
                    .read_fallible(writer)
                    .map_err(|(err, _)| err)?
            };
            inner.data_idx += copied;
            return Ok(Some(copied));
        }

        // 2) Cache empty -> submit exactly one request, if we haven't already.
        if !inner.in_flight {
            let EntropyDeviceInner {
                queue,
                buffer,
                in_flight,
                ..
            } = &mut *inner;

            queue
                .add_dma_buf(&[], &[buffer])
                .map_err(|_| Error::IoError)?;
            if queue.should_notify() {
                queue.notify();
            }
            *in_flight = true;
        }

        Ok(None)
    }

    pub fn wait_queue(&self) -> &WaitQueue {
        &self.wait_queue
    }

    fn handle_recv_irq(&self) {
        let mut inner = self.inner.lock();

        let Ok((_, used_len)) = inner.queue.pop_used() else {
            return;
        };

        let used_len = (used_len as usize).min(inner.buffer.size());
        inner.buffer.sync_from_device(0..used_len).unwrap();
        inner.data_avail = used_len;
        inner.data_idx = 0;
        inner.in_flight = false;
        drop(inner);

        self.wait_queue.wake_all();
    }
}

struct EntropyDeviceInner {
    queue: VirtQueue,
    buffer: DmaStream<FromDevice>,
    data_avail: usize,
    data_idx: usize,
    in_flight: bool,
}

impl EntropyDeviceInner {
    fn new(queue: VirtQueue) -> Self {
        let buffer = DmaStream::<FromDevice>::map(
            FrameAllocOptions::new()
                .alloc_segment(ENTROPY_BUFFER_SIZE / PAGE_SIZE)
                .unwrap()
                .into(),
            false,
        )
        .unwrap();

        Self {
            queue,
            buffer,
            data_avail: 0,
            data_idx: 0,
            in_flight: false,
        }
    }
}
