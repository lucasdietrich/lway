use std::ffi::c_int;

pub fn to_ioresult(ret: c_int) -> std::io::Result<i32> {
    if ret < 0 {
        let error = std::io::Error::last_os_error();
        log::error!("Error: {}", error);
        Err(error)
    } else {
        Ok(ret)
    }
}
