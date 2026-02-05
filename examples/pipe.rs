use libc::{pipe, read, write};

fn main() {
    unsafe {
        let mut pipefd: [i32; 2] = [-1_i32; 2];
        let ret = pipe(pipefd.as_mut_ptr());
        println!("ret: {ret}");
        println!("pipefd: {:?}", pipefd);

        let r = pipefd[0];
        let w = pipefd[1];
        let buf: [u8; 4] = [1, 2, 3, 4];
        let len = size_of_val(&buf);
        let ret = write(w, buf.as_ptr() as *const _, len);
        println!("write: {} {:?}", ret, buf);
        let mut buf: [u8; 4] = [0, 0, 0, 0];
        let ret = read(r, buf.as_mut_ptr() as *mut _, len);
        println!("read: {} {:?}", ret, buf);
    }
}
