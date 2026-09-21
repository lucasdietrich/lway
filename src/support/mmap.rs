use std::{
    fs::File,
    ops::{Deref, DerefMut, Index, IndexMut, Range, RangeFrom, RangeFull, RangeTo},
    os::fd::{AsFd, AsRawFd, RawFd},
};

use crate::support::log_buffer::{MemFdBuffer, Sealed};

fn to_mmap_ioresult(addr: *mut libc::c_void) -> std::io::Result<*mut libc::c_void> {
    if addr == libc::MAP_FAILED {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(addr)
    }
}

/// Size (in bytes) of a page on this system, per `sysconf(_SC_PAGESIZE)`.
pub fn page_size() -> usize {
    unsafe { libc::sysconf(libc::_SC_PAGESIZE) as usize }
}

unsafe fn mmap_ring_buffer(fd: RawFd, size: usize) -> std::io::Result<*mut libc::c_void> {
    let page_size = page_size();

    if size % page_size != 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!(
                "size must be a multiple of the page size (page size = {})",
                page_size
            ),
        ));
    }

    let addr = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            2 * size,
            libc::PROT_NONE,
            libc::MAP_ANONYMOUS | libc::MAP_PRIVATE,
            -1,
            0,
        )
    };
    let addr = to_mmap_ioresult(addr)?;

    let addr1 = unsafe {
        libc::mmap(
            addr,
            size,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_SHARED | libc::MAP_FIXED,
            fd,
            0,
        )
    };
    let addr1 = to_mmap_ioresult(addr1)?;
    assert_eq!(addr, addr1);

    let addr2 = unsafe {
        libc::mmap(
            (addr as usize + size) as *mut libc::c_void,
            size,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_SHARED | libc::MAP_FIXED,
            fd,
            0,
        )
    };
    to_mmap_ioresult(addr2)?;

    Ok(addr)
}

unsafe fn unmap_ring_buffer(addr: *mut libc::c_void, size: usize) {
    libc::munmap(addr, 2 * size);
}

pub struct MmapRingBufferInner {
    addr: *mut libc::c_void, // address range [0, 2 * size[
    size: usize,             // size of half the ring buffer
}

impl MmapRingBufferInner {
    pub unsafe fn new(fd: RawFd, size: usize) -> std::io::Result<Self> {
        let addr = unsafe { mmap_ring_buffer(fd, size) }?;
        Ok(Self { addr, size })
    }
}

impl Drop for MmapRingBufferInner {
    fn drop(&mut self) {
        unsafe {
            unmap_ring_buffer(self.addr, self.size); // size is unknown here, might need adjustment
        }
    }
}

pub struct MmapRingBuffer {
    inner: MmapRingBufferInner,
    file: File,
}

impl MmapRingBuffer {
    pub unsafe fn mmap_file(file: File, size: usize) -> std::io::Result<Self> {
        let inner = unsafe { MmapRingBufferInner::new(file.as_raw_fd(), size) }?;

        Ok(Self { file, inner })
    }

    pub unsafe fn mmap_memfd(memfd: MemFdBuffer<Sealed>) -> std::io::Result<Self> {
        let capacity = memfd.capacity();
        let file = memfd.into_file();
        Self::mmap_file(file, capacity)
    }

    pub fn as_file(&self) -> &File {
        &self.file
    }

    pub fn as_file_mut(&mut self) -> &mut File {
        &mut self.file
    }

    pub fn into_file(self) -> File {
        self.file
    }

    pub fn size(&self) -> usize {
        self.inner.size
    }

    fn get_buffer_infos_at(&self, offset: usize) -> (*const u8, usize) {
        let offset = offset % self.inner.size;
        let addr = unsafe { (self.inner.addr as *const u8).add(offset) };
        (addr, self.inner.size)
    }

    pub fn get_at(&self, offset: usize) -> &[u8] {
        let (addr, size) = self.get_buffer_infos_at(offset);
        unsafe { std::slice::from_raw_parts(addr, size) }
    }

    pub fn get_at_mut(&self, offset: usize) -> &mut [u8] {
        let (addr, size) = self.get_buffer_infos_at(offset);
        unsafe { std::slice::from_raw_parts_mut(addr as *mut u8, size) }
    }
}

impl Index<Range<usize>> for MmapRingBuffer {
    type Output = [u8];

    fn index(&self, index: Range<usize>) -> &Self::Output {
        let buffer = self.get_at(index.start);
        &buffer[..index.end - index.start]
    }
}

impl Index<RangeFrom<usize>> for MmapRingBuffer {
    type Output = [u8];

    fn index(&self, index: RangeFrom<usize>) -> &Self::Output {
        let buffer = self.get_at(index.start);
        &buffer[..]
    }
}

impl Index<RangeTo<usize>> for MmapRingBuffer {
    type Output = [u8];

    fn index(&self, index: RangeTo<usize>) -> &Self::Output {
        let buffer = self.get_at(0);
        &buffer[..index.end]
    }
}

impl Index<RangeFull> for MmapRingBuffer {
    type Output = [u8];

    fn index(&self, _index: RangeFull) -> &Self::Output {
        self.get_at(0)
    }
}

impl IndexMut<Range<usize>> for MmapRingBuffer {
    fn index_mut(&mut self, index: Range<usize>) -> &mut Self::Output {
        let buffer = self.get_at_mut(index.start);
        &mut buffer[..index.end - index.start]
    }
}

impl IndexMut<RangeFrom<usize>> for MmapRingBuffer {
    fn index_mut(&mut self, index: RangeFrom<usize>) -> &mut Self::Output {
        let buffer = self.get_at_mut(index.start);
        &mut buffer[..]
    }
}

impl IndexMut<RangeTo<usize>> for MmapRingBuffer {
    fn index_mut(&mut self, index: RangeTo<usize>) -> &mut Self::Output {
        let buffer = self.get_at_mut(0);
        &mut buffer[..index.end]
    }
}

impl IndexMut<RangeFull> for MmapRingBuffer {
    fn index_mut(&mut self, _index: RangeFull) -> &mut Self::Output {
        self.get_at_mut(0)
    }
}

impl Deref for MmapRingBuffer {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        self.get_at(0)
    }
}

impl DerefMut for MmapRingBuffer {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.get_at_mut(0)
    }
}

impl AsFd for MmapRingBuffer {
    fn as_fd(&self) -> std::os::fd::BorrowedFd<'_> {
        self.file.as_fd()
    }
}

impl AsRawFd for MmapRingBuffer {
    fn as_raw_fd(&self) -> std::os::fd::RawFd {
        self.file.as_raw_fd()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fill_pattern(mmap_rb: &mut MmapRingBuffer) {
        let buf = &mut mmap_rb[..];
        for (i, byte) in buf.iter_mut().enumerate() {
            *byte = (i % 256) as u8;
        }
    }

    #[test]
    fn test_mmap_ring_buffer() {
        let capacity = 3 * 4096; // Example capacity, you can adjust as needed
        let memfd = MemFdBuffer::new_sealed("unamedf", capacity).unwrap();
        let mmap_ring_buffer = unsafe { MmapRingBuffer::mmap_memfd(memfd).unwrap() };

        assert_eq!(mmap_ring_buffer.size(), capacity);
    }

    #[test]
    fn test_mmap_ring_buffer_invalid_capacity() {
        let capacity = 1000;
        let memfd = MemFdBuffer::new_sealed("unamedf", capacity).unwrap();
        let result = unsafe { MmapRingBuffer::mmap_memfd(memfd) };
        assert!(result.is_err());
    }

    #[test]
    fn test_mmap_ring_buffer_zero_capacity() {
        let capacity = 0;
        let memfd = MemFdBuffer::new_sealed("unamedf", capacity).unwrap();
        let result = unsafe { MmapRingBuffer::mmap_memfd(memfd) };
        assert!(result.is_err());
    }

    #[test]
    fn test_mmap_ring_buffer_range() {
        let pgsize = 4096;
        let capacity = 3 * pgsize;
        let offset = 3000;
        let memfd = MemFdBuffer::new_sealed("unamedf", capacity).unwrap();
        let mut mmap_ring_buffer = unsafe { MmapRingBuffer::mmap_memfd(memfd).unwrap() };

        fill_pattern(&mut mmap_ring_buffer);

        // get full buffer
        let range1 = &mmap_ring_buffer[..];
        assert_eq!(range1.len(), capacity);

        // get buffer starting from offset
        let range2 = &mmap_ring_buffer[offset..];
        assert_eq!(range2.len(), capacity);

        // get buffer starting from offset + capacity * n
        let n = 3; // can be any value
        let range3 = &mmap_ring_buffer[(offset + capacity * n)..];
        assert_eq!(range3.len(), capacity);

        // verify that slicing with offset works as expected
        assert_eq!(range1[offset..].len(), capacity - offset);
        assert_eq!(range2[..capacity - offset].len(), capacity - offset);
        assert_eq!(range3[..capacity - offset].len(), capacity - offset);
        assert_eq!(&range1[offset..], &range2[..capacity - offset]);
        assert_eq!(&range1[offset..], &range3[..capacity - offset]);
    }
}
