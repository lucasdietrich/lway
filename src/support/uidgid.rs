use std::{
    ffi::{CStr, CString},
    path::PathBuf,
};

fn get_cwd_path<'a>(buf: &'a mut impl AsMut<[u8]>) -> std::io::Result<&'a CStr> {
    let buf = buf.as_mut();
    let ret = unsafe { libc::getcwd(buf.as_mut_ptr() as *mut libc::c_char, buf.len()) };
    if ret.is_null() {
        Err(std::io::Error::last_os_error())
    } else {
        // getcwd only null-terminates at the actual length; stop at the first NUL
        // instead of treating the whole (zero-initialized) buffer as the string.
        Ok(unsafe { CStr::from_ptr(buf.as_ptr() as *const libc::c_char) })
    }
}

// std::env::current_dir() rewrite because I find std::sys::paths::unix::getcwd() impl terrible
pub fn get_current_cwd() -> std::io::Result<PathBuf> {
    let mut buf = [0u8; libc::PATH_MAX as usize];
    let cwd = get_cwd_path(&mut buf)?;
    Ok(PathBuf::from(cwd.to_string_lossy().into_owned()))
}

pub fn set_current_cwd(path: &PathBuf) -> std::io::Result<()> {
    let path_string = path.display().to_string();
    let path_cstr = CString::new(path_string).expect("cwd");
    let ret = unsafe { libc::chdir(path_cstr.as_ptr()) };
    if ret == -1 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

pub fn get_current_uid() -> u32 {
    unsafe { libc::geteuid() }
}

pub fn get_current_gid() -> u32 {
    unsafe { libc::getegid() }
}

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

pub fn get_username(uid: u32) -> Option<String> {
    unsafe {
        let pw = libc::getpwuid(uid);
        if pw.is_null() {
            None
        } else {
            Some(CStr::from_ptr((*pw).pw_name).to_string_lossy().into_owned())
        }
    }
}

pub fn get_groupname(gid: u32) -> Option<String> {
    unsafe {
        let gr = libc::getgrgid(gid);
        if gr.is_null() {
            None
        } else {
            Some(CStr::from_ptr((*gr).gr_name).to_string_lossy().into_owned())
        }
    }
}
