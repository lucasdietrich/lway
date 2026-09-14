use std::ffi::CString;

pub fn get_uid(username: &str) -> Option<u32> {
    let c_username = CString::new(username).ok()?;
    unsafe {
        let pw = libc::getpwnam(c_username.as_ptr());
        if pw.is_null() {
            None
        } else {
            Some((*pw).pw_uid)
        }
    }
}

pub fn get_gid(groupname: &str) -> Option<u32> {
    let c_groupname = CString::new(groupname).ok()?;
    unsafe {
        let gr = libc::getgrnam(c_groupname.as_ptr());
        if gr.is_null() {
            None
        } else {
            Some((*gr).gr_gid)
        }
    }
}
