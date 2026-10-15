**Role:** You are an expert Rust systems programmer specializing in the Asterinas framework and Virtio specifications.

**Context:** I am implementing the `virtio-crypto` device driver (Virtio V1.3 Spec, Section 5.9) within the **Asterinas** OSKernel. Asterinas uses a safe, modular Rust architecture. We need to implement the core device structures, control queue handling, and data queue processing for cryptographic services.

**Objective:** Generate the Rust implementation for the `virtio-crypto` device, following the Asterinas driver model.

**Requirements:**

1. **Consistency:** The code must be compatible with the Asterinas kernel environment, and try to use existing api in Asterinas.
    
2. **Virtio Structs:** Define the `virtio_crypto_config`, `virtio_crypto_hdr`, and request structures as per the V1.3 spec. Use `zerocopy` or `volatile` if necessary for memory safety.
    
3. **Queue Management:** Implement handling for the `controlq` (for session creation/closing) and `dataq` (for encryption/decryption operations).
    
4. **Asynchronous Handling:** Follow the Asterinas asynchronous interrupt-driven model. Use `Waker` patterns common in the existing Asterinas Virtio implementations (e.g., like `virtio-blk` or `virtio-fs`).
    

**Reference Structure:**

- `CryptoDevice`: The main struct holding the Virtio queues.
    
- `CryptoHeader`: For request metadata.
    
- `Session`: Logic to manage crypto sessions.
    

**Please start by breaking down the whole task and giving me a plan to review**