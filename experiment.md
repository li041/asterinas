# Experiment Log: Virtio-9P + Virtio-Crypto (Agent-Assisted)

## 1. Scope and Objectives

This experiment covered two tracks:

1. Virtio-9P bring-up for shared-directory mount and basic file/directory operations.
2. Initial virtio-crypto bring-up with `/dev/crypto` compatibility and AES-CBC cipher path.

Constraints used throughout the work:

- Reuse existing Asterinas virtio architecture patterns as much as possible.
- Avoid framework-wide refactors; prefer local, reversible changes.

## 2. Original Prompt

Role: You are an expert Rust systems programmer specializing in the Asterinas framework and Virtio specifications.

Context: I am implementing the `virtio-crypto` device driver (Virtio V1.3 Spec, Section 5.9) within the **Asterinas** OSKernel. Asterinas uses a safe, modular Rust architecture. We need to implement the core device structures, control queue handling, and data queue processing for cryptographic services.

Objective: Generate the Rust implementation for the `virtio-crypto` device, following the Asterinas driver model.

Requirements:

1. Consistency: The code must be compatible with the Asterinas kernel environment, and try to use existing api in Asterinas.
2. Virtio Structs: Define the `virtio_crypto_config`, `virtio_crypto_hdr`, and request structures as per the V1.3 spec.
3. Queue Management: Implement handling for the `controlq` (for session creation/closing) and `dataq` (for encryption/decryption operations).
4. Asynchronous Handling: Follow the Asterinas asynchronous interrupt-driven model. Use `Waker` patterns common in the existing Asterinas Virtio implementations (e.g., like `virtio-blk` or `virtio-fs`).

Reference Structure:
- `CryptoDevice`: The main struct holding the Virtio queues.
- `CryptoHeader`: For request metadata.
- `Session`: Logic to manage crypto sessions.

Please start by breaking down the whole task and giving me a plan to review.

## 3. Timeline with Reasoning and Design Decisions

### Phase A: Virtio-9P Baseline and Reproducible Mount Path

- Completed the virtio-9p mount path for shared directories and validated basic file/directory operations.
- Synced the integration style with virtio-fs layering to keep responsibilities clean between transport and upper fs integration.
- Defined reproducible verification steps early so later protocol iterations could be evaluated with stable checks.

### Phase B: Initial Virtio-Crypto Device and `/dev/crypto` Compatibility

- Added the virtio-crypto device skeleton, queue initialization, and callback registration in the virtio component layer.
- Added kernel wrapper and misc char-device bridge to expose `/dev/crypto` for cryptodev userspace tests.
- Implemented the first ioctl set (`CRIOGET`, `CIOCGSESSION`, `CIOCGSESSINFO`, `CIOCCRYPT`, `CIOCFSESSION`) to establish an end-to-end runnable path.
- Chose localized compatibility bridging instead of framework refactor to match the “minimal invasive change” constraint.

### Phase C: Protocol Convergence Driven by Runtime Errors

- Investigated protocol-level errors from QEMU and treated them as on-wire evidence instead of generic runtime failures.
- Cross-checked implementation details against Linux UAPI `virtio_crypto.h` and Linux virtio-crypto driver behavior.
- Fixed wire-compatibility issues in sequence:
   - request header size/field layout,
   - control/data queue routing,
   - destroy-session status width (`inhdr.status`, 1 byte),
   - session lifecycle alignment for encrypt/decrypt paths.

### Phase D: Generalizing Beyond AES-only Branching

- Used `speed` failure on `CRYPTO_NULL` as the trigger to move away from AES-only branching.
- Introduced an algorithm-spec + backend-dispatch structure so ioctl flow stays stable while algorithms grow incrementally.
- Kept AES-CBC on virtio backend and added `CRYPTO_NULL` passthrough path as a compatibility baseline.

## 5. Main Problems and How They Were Addressed
1. Protocol mismatch errors at runtime.
   - Approach: field-by-field verification against Linux API
2. Queue semantic mismatch.
   - Approach: map opcode symptom patterns back to queue routing and patch minimally.
3. Session close failures.
   - Approach: align destroy request/response semantics with Linux behavior.
4. Benchmark path blocked by rigid algorithm handling.
   - Approach: introduce cipher spec + backend abstraction to decouple ioctl flow from specific algorithms.

