pub mod log_buffer;
pub mod mio_token_slab;
pub mod mmap;
pub mod pid;
pub mod pipe;
pub mod signal;
pub mod uidgid;

use std::ffi::c_int;

pub const KB: usize = 1024;
pub const MB: usize = 1024 * KB;
pub const GB: usize = 1024 * MB;

pub fn to_ioresult(ret: c_int) -> std::io::Result<i32> {
    if ret < 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(ret)
    }
}
