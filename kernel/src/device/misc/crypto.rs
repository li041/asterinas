// SPDX-License-Identifier: MPL-2.0

use alloc::sync::Arc;

use device_id::{DeviceId, MinorId};
use ostd::{mm::VmIo, task::Task};

use super::MISC_MAJOR;
use crate::{
    crypto::virtio::{CryptoSession, VirtioCrypto},
    fs::{
        device::{Device, DeviceType},
        file_table::{FdFlags, FileDesc},
        inode_handle::FileIo,
        path::FsPath,
        utils::{AccessMode, InodeIo, OpenArgs, StatusFlags, mkmod},
    },
    prelude::*,
    process::{
        posix_thread::AsThreadLocal,
        signal::{PollHandle, Pollable},
    },
    util::ioctl::{RawIoctl, dispatch_ioctl},
};

const CRYPTO_MINOR: u32 = 70;
const CRYPTO_NULL: u32 = 16;
const CRYPTO_AES_CBC: u32 = 11;
const COP_ENCRYPT: u16 = 0;
const COP_DECRYPT: u16 = 1;
const COP_FLAG_NONE: u16 = 0;

const VIRTIO_CRYPTO_CIPHER_AES_CBC: u32 = 3;
const AES_BLOCK_SIZE: usize = 16;

#[derive(Debug)]
pub struct CryptoDevice {
    id: DeviceId,
}

impl CryptoDevice {
    pub fn new() -> Arc<Self> {
        let major = MISC_MAJOR.get().unwrap().get();
        let minor = MinorId::new(CRYPTO_MINOR);
        let id = DeviceId::new(major, minor);
        Arc::new(Self { id })
    }
}

impl Device for CryptoDevice {
    fn type_(&self) -> DeviceType {
        DeviceType::Char
    }

    fn id(&self) -> DeviceId {
        self.id
    }

    fn devtmpfs_path(&self) -> Option<String> {
        Some("crypto".into())
    }

    fn open(&self) -> Result<Box<dyn FileIo>> {
        Ok(Box::new(CryptoFile {
            next_ses: Mutex::new(1),
            sessions: Mutex::new(BTreeMap::new()),
        }))
    }
}

struct SessionState {
    cipher: u32,
    iv_len: usize,
    backend: CipherBackend,
}

enum CipherBackend {
    Null,
    Virtio(CryptoSession),
}

impl CipherBackend {
    fn crypt(&self, op: u16, iv: &[u8], src: &[u8]) -> Result<Vec<u8>> {
        match self {
            Self::Null => Ok(src.to_vec()),
            Self::Virtio(session) => match op {
                COP_ENCRYPT => session.encrypt(iv, src),
                COP_DECRYPT => session.decrypt(iv, src),
                _ => return_errno_with_message!(Errno::EINVAL, "invalid crypt_op op"),
            },
        }
    }

    fn close(self) -> Result<()> {
        match self {
            Self::Null => Ok(()),
            Self::Virtio(session) => session.close(),
        }
    }
}

struct CipherSpec {
    virtio_algo: Option<u32>,
    iv_len: usize,
    min_key_len: usize,
    max_key_len: usize,
}

fn cipher_spec(cipher: u32) -> Option<CipherSpec> {
    match cipher {
        CRYPTO_NULL => Some(CipherSpec {
            virtio_algo: None,
            iv_len: 0,
            min_key_len: 0,
            max_key_len: 0,
        }),
        CRYPTO_AES_CBC => Some(CipherSpec {
            virtio_algo: Some(VIRTIO_CRYPTO_CIPHER_AES_CBC),
            iv_len: AES_BLOCK_SIZE,
            min_key_len: 16,
            max_key_len: 32,
        }),
        _ => None,
    }
}

fn fill_cipher_info(cipher: u32, info: &mut SessionInfoOp) {
    match cipher {
        CRYPTO_NULL => {
            fill_cstr(&mut info.cipher_info.cra_name, b"ecb(cipher_null)");
            fill_cstr(&mut info.cipher_info.cra_driver_name, b"cryptodev-null");
        }
        CRYPTO_AES_CBC => {
            fill_cstr(&mut info.cipher_info.cra_name, b"cbc(aes)");
            fill_cstr(
                &mut info.cipher_info.cra_driver_name,
                b"virtio-crypto-aes-cbc",
            );
        }
        _ => {
            fill_cstr(&mut info.cipher_info.cra_name, b"unknown");
            fill_cstr(&mut info.cipher_info.cra_driver_name, b"unknown");
        }
    }
}

struct CryptoFile {
    next_ses: Mutex<u32>,
    sessions: Mutex<BTreeMap<u32, SessionState>>,
}

impl Pollable for CryptoFile {
    fn poll(
        &self,
        mask: crate::events::IoEvents,
        _poller: Option<&mut PollHandle>,
    ) -> crate::events::IoEvents {
        let events = crate::events::IoEvents::IN | crate::events::IoEvents::OUT;
        events & mask
    }
}

impl InodeIo for CryptoFile {
    fn read_at(
        &self,
        _offset: usize,
        _writer: &mut VmWriter,
        _status_flags: StatusFlags,
    ) -> Result<usize> {
        return_errno_with_message!(Errno::EINVAL, "read is not supported on /dev/crypto")
    }

    fn write_at(
        &self,
        _offset: usize,
        _reader: &mut VmReader,
        _status_flags: StatusFlags,
    ) -> Result<usize> {
        return_errno_with_message!(Errno::EINVAL, "write is not supported on /dev/crypto")
    }
}

impl FileIo for CryptoFile {
    fn check_seekable(&self) -> Result<()> {
        return_errno_with_message!(Errno::ESPIPE, "seek is not supported")
    }

    fn is_offset_aware(&self) -> bool {
        false
    }

    fn ioctl(&self, raw_ioctl: RawIoctl) -> Result<i32> {
        use ioctl_defs::*;

        dispatch_ioctl!(match raw_ioctl {
            cmd @ CrioGet => {
                let new_fd = self.clone_fd()?;
                cmd.write(&(new_fd as u32))?;
                Ok(0)
            }
            cmd @ CiocgSession => {
                cmd.with_data_ptr(|data_ptr| {
                    let mut op = data_ptr.read()?;
                    let ses = self.create_session(&op)?;
                    op.ses = ses;
                    data_ptr.write(&op)?;
                    Ok(0)
                })
            }
            cmd @ CiocfSession => {
                let ses = cmd.read()?;
                self.close_session(ses)?;
                Ok(0)
            }
            cmd @ CiocCrypt => {
                cmd.with_data_ptr(|data_ptr| {
                    let op = data_ptr.read()?;
                    self.crypt(&op)?;
                    Ok(0)
                })
            }
            cmd @ CiocgSessInfo => {
                cmd.with_data_ptr(|data_ptr| {
                    let mut info = data_ptr.read()?;
                    self.fill_sess_info(&mut info)?;
                    data_ptr.write(&info)?;
                    Ok(0)
                })
            }
            _ => return_errno_with_message!(Errno::ENOTTY, "the ioctl command is unknown"),
        })
    }
}

impl CryptoFile {
    fn clone_fd(&self) -> Result<FileDesc> {
        let task =
            Task::current().ok_or_else(|| Error::with_message(Errno::ESRCH, "no current task"))?;
        let thread_local = AsThreadLocal::as_thread_local(&task)
            .ok_or_else(|| Error::with_message(Errno::ESRCH, "no current thread local"))?;

        let path = {
            let fs = thread_local.borrow_fs();
            let resolver = fs.resolver().read();
            resolver.lookup(&FsPath::try_from("/dev/crypto")?)?
        };

        let open_args = OpenArgs::from_modes(AccessMode::O_RDWR, mkmod!(a+rw));
        let file = Arc::new(path.open(open_args)?);

        let mut file_table_ref = thread_local.borrow_file_table_mut();
        let mut file_table = file_table_ref.unwrap().write();
        Ok(file_table.insert(file, FdFlags::empty()))
    }

    fn create_session(&self, op: &SessionOp) -> Result<u32> {
        let spec = cipher_spec(op.cipher)
            .ok_or_else(|| Error::with_message(Errno::EOPNOTSUPP, "cipher is not supported"))?;

        let keylen = op.keylen as usize;
        if keylen < spec.min_key_len || keylen > spec.max_key_len {
            return_errno_with_message!(Errno::EINVAL, "invalid key length for selected cipher");
        }

        let backend = if let Some(virtio_algo) = spec.virtio_algo {
            if op.key == 0 {
                return_errno_with_message!(
                    Errno::EINVAL,
                    "invalid key pointer for selected cipher"
                );
            }

            let mut key = vec![0u8; keylen];
            crate::current_userspace!().read_bytes(op.key, &mut key)?;

            let crypto = VirtioCrypto::new_default()?;
            let session = crypto.open_cipher_session(virtio_algo, &key, spec.iv_len as u32)?;
            CipherBackend::Virtio(session)
        } else {
            CipherBackend::Null
        };

        let mut next = self.next_ses.lock();
        let ses = *next;
        *next = next.wrapping_add(1);
        if *next == 0 {
            *next = 1;
        }

        self.sessions.lock().insert(
            ses,
            SessionState {
                cipher: op.cipher,
                iv_len: spec.iv_len,
                backend,
            },
        );

        Ok(ses)
    }

    fn close_session(&self, ses: u32) -> Result<()> {
        let state = self
            .sessions
            .lock()
            .remove(&ses)
            .ok_or_else(|| Error::with_message(Errno::ENOENT, "session not found"))?;
        state.backend.close()?;
        Ok(())
    }

    fn crypt(&self, op: &CryptOp) -> Result<()> {
        let sessions = self.sessions.lock();
        let state = sessions
            .get(&op.ses)
            .ok_or_else(|| Error::with_message(Errno::ENOENT, "session not found"))?;

        if op.flags != COP_FLAG_NONE {
            return_errno_with_message!(
                Errno::EOPNOTSUPP,
                "crypt_op flags are not supported currently"
            );
        }

        if state.cipher == CRYPTO_AES_CBC && op.len as usize % AES_BLOCK_SIZE != 0 {
            return_errno_with_message!(Errno::EINVAL, "AES-CBC length must be block aligned");
        }

        if op.src == 0 || op.dst == 0 {
            return_errno_with_message!(Errno::EINVAL, "invalid crypt_op pointers");
        }
        if state.iv_len != 0 && op.iv == 0 {
            return_errno_with_message!(Errno::EINVAL, "invalid IV pointer for selected cipher");
        }

        let mut src = vec![0u8; op.len as usize];
        crate::current_userspace!().read_bytes(op.src, &mut src)?;

        let mut iv = vec![0u8; state.iv_len];
        if state.iv_len != 0 {
            crate::current_userspace!().read_bytes(op.iv, &mut iv)?;
        }

        let out = state.backend.crypt(op.op, &iv, &src)?;

        if !matches!(op.op, COP_ENCRYPT | COP_DECRYPT) {
            return_errno_with_message!(Errno::EINVAL, "invalid crypt_op op");
        };

        if out.len() != src.len() {
            return_errno_with_message!(Errno::EIO, "unexpected output length");
        }

        crate::current_userspace!().write_bytes(op.dst, &out)?;
        Ok(())
    }

    fn fill_sess_info(&self, info: &mut SessionInfoOp) -> Result<()> {
        let sessions = self.sessions.lock();
        let state = sessions
            .get(&info.ses)
            .ok_or_else(|| Error::with_message(Errno::ENOENT, "session not found"))?;

        fill_cipher_info(state.cipher, info);
        info.alignmask = 0;
        info.flags = 0;
        Ok(())
    }
}

fn fill_cstr(dst: &mut [u8], src: &[u8]) {
    let copy_len = core::cmp::min(dst.len().saturating_sub(1), src.len());
    dst[..copy_len].copy_from_slice(&src[..copy_len]);
    dst[copy_len] = 0;
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod)]
struct SessionOp {
    cipher: u32,
    mac: u32,
    keylen: u32,
    _pad0: u32,
    key: Vaddr,
    mackeylen: u32,
    _pad1: u32,
    mackey: Vaddr,
    ses: u32,
    _pad2: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod)]
struct AlgInfo {
    cra_name: [u8; 64],
    cra_driver_name: [u8; 64],
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod)]
struct SessionInfoOp {
    ses: u32,
    cipher_info: AlgInfo,
    hash_info: AlgInfo,
    alignmask: u16,
    _pad0: u16,
    flags: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod)]
struct CryptOp {
    ses: u32,
    op: u16,
    flags: u16,
    len: u32,
    _pad0: u32,
    src: Vaddr,
    dst: Vaddr,
    mac: Vaddr,
    iv: Vaddr,
}

mod ioctl_defs {
    use super::{CryptOp, SessionInfoOp, SessionOp};
    use crate::util::ioctl::{InData, InOutData, ioc};

    pub(super) type CrioGet = ioc!(CRIOGET, b'c', 101, InOutData<u32>);
    pub(super) type CiocgSession = ioc!(CIOCGSESSION, b'c', 102, InOutData<SessionOp>);
    pub(super) type CiocfSession = ioc!(CIOCFSESSION, b'c', 103, InData<u32>);
    pub(super) type CiocCrypt = ioc!(CIOCCRYPT, b'c', 104, InOutData<CryptOp>);
    pub(super) type CiocgSessInfo = ioc!(CIOCGSESSINFO, b'c', 107, InOutData<SessionInfoOp>);
}
