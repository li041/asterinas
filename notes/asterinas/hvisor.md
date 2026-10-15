- 

## Changes Made
  1. src/hypercall/mod.rs — Response-side IPI batching
	- hv_virtio_inject_irq() now collects target CPUs in a u64 bitmap (ipi_targets) during the response processing loop
	- IPIs are sent in a separate loop after all responses are processed, ensuring each CPU receives at most one IPI
	- If 10 responses target 2 CPUs, IPI count drops from 10 to 2

  2. src/device/virtio_trampoline.rs — Request-side IPI dedup
	  - Added pub ipi_pending: AtomicBool field to VirtioBridgeController
	  - check_need_wakeup_and_send_ipi() now uses compare_exchange(false, true) to ensure only one CPU wins the race and sends the wakeup IPI
	  - Other CPUs that also detect need_wakeup will skip the IPI send but still set is_send_ipi = true to avoid re-checking

  3. src/event.rs — Clear ipi_pending on wakeup delivery
	  - Added VIRTIO_BRIDGE import
	  - In the IPI_EVENT_WAKEUP_VIRTIO_DEVICE handler, ipi_pending is cleared with Release ordering before injecting the IRQ, allowing future wakeup IPIs to be sent