// SPDX-License-Identifier: MPL-2.0

use int_to_c_enum::TryFromInt;

pub const VIRTIO_CRYPTO_SERVICE_CIPHER: u32 = 0;
pub const VIRTIO_CRYPTO_SERVICE_CIPHER_MASK: u32 = 1 << VIRTIO_CRYPTO_SERVICE_CIPHER;

const fn virtio_crypto_opcode(service: u32, op: u32) -> u32 {
    (service << 8) | op
}

pub const VIRTIO_CRYPTO_OK: u32 = 0;

pub const VIRTIO_CRYPTO_CIPHER_CREATE_SESSION: u32 =
    virtio_crypto_opcode(VIRTIO_CRYPTO_SERVICE_CIPHER, 0x02);
pub const VIRTIO_CRYPTO_CIPHER_DESTROY_SESSION: u32 =
    virtio_crypto_opcode(VIRTIO_CRYPTO_SERVICE_CIPHER, 0x03);
pub const VIRTIO_CRYPTO_CIPHER_ENCRYPT: u32 =
    virtio_crypto_opcode(VIRTIO_CRYPTO_SERVICE_CIPHER, 0x00);
pub const VIRTIO_CRYPTO_CIPHER_DECRYPT: u32 =
    virtio_crypto_opcode(VIRTIO_CRYPTO_SERVICE_CIPHER, 0x01);

pub const VIRTIO_CRYPTO_SYM_OP_CIPHER: u32 = 1;
pub const VIRTIO_CRYPTO_OP_ENCRYPT: u32 = 1;
pub const VIRTIO_CRYPTO_OP_DECRYPT: u32 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, TryFromInt)]
#[repr(u32)]
pub enum VirtioCryptoStatus {
    Ok = 0,
    Err = 1,
    BadMsg = 2,
    NotSupp = 3,
    InvParam = 4,
}

#[repr(C)]
#[derive(Debug, Pod, Clone, Copy)]
pub struct VirtioCryptoCtrlHeader {
    pub opcode: u32,
    pub algo: u32,
    pub flag: u32,
    pub queue_id: u32,
}

#[repr(C)]
#[derive(Debug, Pod, Clone, Copy)]
pub struct VirtioCryptoOpHeader {
    pub opcode: u32,
    pub algo: u32,
    pub session_id: u64,
    pub flag: u32,
    pub padding: u32,
}

#[allow(non_camel_case_types)]
pub type virtio_crypto_hdr = VirtioCryptoOpHeader;

#[repr(C)]
#[derive(Debug, Pod, Clone, Copy)]
pub struct VirtioCryptoSessionInput {
    pub session_id: u64,
    pub status: u32,
    pub padding: u32,
}

#[repr(C)]
#[derive(Debug, Pod, Clone, Copy)]
pub struct VirtioCryptoCipherSessionPara {
    pub algo: u32,
    pub keylen: u32,
    pub op: u32,
    pub padding: u32,
}

#[repr(C)]
#[derive(Debug, Pod, Clone, Copy)]
pub struct VirtioCryptoCreateSessionReq {
    pub header: VirtioCryptoCtrlHeader,
    pub para: VirtioCryptoCipherSessionPara,
    pub cipher_padding: [u8; 32],
    pub op_type: u32,
    pub padding: u32,
}

#[repr(C)]
#[derive(Debug, Pod, Clone, Copy)]
pub struct VirtioCryptoDestroySessionReq {
    pub header: VirtioCryptoCtrlHeader,
    pub session_id: u64,
    pub padding: [u8; 48],
}

#[repr(C)]
#[derive(Debug, Pod, Clone, Copy)]
pub struct VirtioCryptoCipherDataReq {
    pub header: VirtioCryptoOpHeader,
    pub iv_len: u32,
    pub src_len: u32,
    pub dst_len: u32,
    pub para_padding: u32,
    pub cipher_padding: [u8; 24],
    pub op_type: u32,
    pub req_padding: u32,
}

#[repr(C)]
#[derive(Debug, Pod, Clone, Copy)]
pub struct VirtioCryptoInHdr {
    pub status: u8,
}

#[repr(C)]
#[derive(Debug, Pod, Clone, Copy)]
pub struct VirtioCryptoDataStatus {
    pub status: u8,
}

#[cfg(test)]
mod tests {
    use core::mem::{align_of, size_of};

    use super::*;

    #[test]
    fn crypto_wire_layout() {
        assert_eq!(size_of::<VirtioCryptoOpHeader>(), 24);
        assert_eq!(align_of::<VirtioCryptoOpHeader>(), 8);
        assert_eq!(size_of::<VirtioCryptoCtrlHeader>(), 16);
        assert_eq!(size_of::<VirtioCryptoSessionInput>(), 16);
        assert_eq!(size_of::<VirtioCryptoCreateSessionReq>(), 72);
        assert_eq!(size_of::<VirtioCryptoDestroySessionReq>(), 72);
        assert_eq!(size_of::<VirtioCryptoCipherDataReq>(), 72);
    }
}
