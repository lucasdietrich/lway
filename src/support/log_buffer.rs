use std::{
    ffi::CString,
    fs::File,
    io::Seek,
    os::fd::{AsFd, AsRawFd, FromRawFd, RawFd},
};

use memmap2::MmapMut;

use crate::{
    logger::{LogBuffer, LogChunk, ViewableLogBuffer},
    support::{mmap::MmapRingBuffer, pipe::splice, to_ioresult, KB},
};

pub const DEFAULT_LOG_BUFFER_SIZE: usize = 64 * KB;

pub trait SealedTrait {}

pub struct Sealed {
    capacity: usize,
}

impl SealedTrait for Sealed {}

pub struct Unsealed {}

impl SealedTrait for Unsealed {}

pub struct MemFdBuffer<T: SealedTrait = Unsealed> {
    file: File,
    sealed: T,
}

impl MemFdBuffer {
    fn memfd_create(name: &str) -> std::io::Result<File> {
        let name = std::ffi::CString::new(name).unwrap();
        let flags = libc::MFD_NOEXEC_SEAL | libc::MFD_ALLOW_SEALING;
        let ret = unsafe { libc::memfd_create(name.as_ptr(), flags) };
        let fd = to_ioresult(ret)?;
        // own the fd immediately so it's closed if a later step fails
        let file = unsafe { File::from_raw_fd(fd) };
        Ok(file)
    }

    pub fn new(name: &str) -> std::io::Result<MemFdBuffer> {
        let file = Self::memfd_create(name)?;
        Ok(MemFdBuffer {
            file,
            sealed: Unsealed {},
        })
    }

    pub fn new_sealed(name: &str, capacity: usize) -> std::io::Result<MemFdBuffer<Sealed>> {
        let file = Self::memfd_create(name)?;

        let fd = file.as_raw_fd();
        let ret = unsafe { libc::ftruncate(fd, capacity as libc::off_t) };
        to_ioresult(ret)?;

        let seals = libc::F_SEAL_SHRINK | libc::F_SEAL_GROW;
        let ret = unsafe { libc::fcntl(fd, libc::F_ADD_SEALS, seals) };
        to_ioresult(ret)?;

        Ok(MemFdBuffer {
            file,
            sealed: Sealed { capacity },
        })
    }
}

impl MemFdBuffer<Sealed> {
    pub fn capacity(&self) -> usize {
        self.sealed.capacity
    }
}

impl<T: SealedTrait> MemFdBuffer<T> {
    pub fn into_file(self) -> File {
        self.file
    }
}

impl LogBuffer for MemFdBuffer<Sealed> {
    fn as_file(&self) -> &File {
        &self.file
    }

    fn filling(&mut self) -> usize {
        self.as_file()
            .stream_position()
            .expect("Failed to get stream position") as usize
    }

    fn capacity(&mut self) -> Option<usize> {
        Some(self.sealed.capacity)
    }

    fn kind(&self) -> &'static str {
        "memfd-sealed"
    }
}

impl LogBuffer for MemFdBuffer<Unsealed> {
    fn as_file(&self) -> &File {
        &self.file
    }

    fn filling(&mut self) -> usize {
        self.as_file()
            .stream_position()
            .expect("Failed to get stream position") as usize
    }

    fn capacity(&mut self) -> Option<usize> {
        None
    }

    fn kind(&self) -> &'static str {
        "memfd"
    }
}

impl<T: SealedTrait> AsFd for MemFdBuffer<T> {
    fn as_fd(&self) -> std::os::fd::BorrowedFd<'_> {
        self.file.as_fd()
    }
}

impl<T: SealedTrait> AsRawFd for MemFdBuffer<T> {
    fn as_raw_fd(&self) -> std::os::fd::RawFd {
        self.file.as_raw_fd()
    }
}

/// A fixed-capacity `memfd`-backed log buffer that behaves like a ring buffer: once
/// `capacity` bytes have been written, further writes wrap back to the start and
/// overwrite the oldest data instead of growing (the underlying memfd is sealed at a
/// fixed size, so it physically cannot grow).
pub struct CircularMappedMemFdBuffer {
    // file: File,
    mmap: MmapRingBuffer,
    capacity: usize,
    /// Total bytes ever written; the physical write offset is `write_pos % capacity`.
    write_pos: usize,
}

impl CircularMappedMemFdBuffer {
    fn new(mmap: MmapRingBuffer, capacity: usize) -> std::io::Result<Self> {
        Ok(Self {
            // file,
            mmap,
            capacity,
            write_pos: 0,
        })
    }

    pub fn new_from_memfd_buffer(memfd: MemFdBuffer<Sealed>) -> std::io::Result<Self> {
        let capacity = memfd.sealed.capacity;
        let mmap = unsafe { MmapRingBuffer::mmap_memfd(memfd)? };
        Self::new(mmap, capacity)
    }

    pub fn new_from_file(file: File, capacity: usize) -> std::io::Result<Self> {
        let mmap = unsafe { MmapRingBuffer::mmap_file(file, capacity)? };
        Self::new(mmap, capacity)
    }

    pub fn into_file(self) -> File {
        self.mmap.into_file()
    }

    pub fn get_mmap(&mut self) -> &mut MmapRingBuffer {
        &mut self.mmap
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Total bytes ever written to the buffer (monotonic, doesn't wrap).
    pub fn write_pos(&self) -> usize {
        self.write_pos
    }

    /// Offset of the oldest byte still retained; data before this has been overwritten.
    pub fn start_pos(&self) -> usize {
        self.write_pos.saturating_sub(self.capacity)
    }

    pub fn wrap_count(&self) -> usize {
        self.write_pos / self.capacity
    }
}

impl LogBuffer for CircularMappedMemFdBuffer {
    fn as_file(&self) -> &File {
        self.mmap.as_file()
    }

    fn splice_from(&mut self, read_fd: RawFd, len: usize) -> std::io::Result<usize> {
        match self.splice_from_and_view(read_fd, len)? {
            Some(slice) => Ok(slice.len()),
            None => Ok(0),
        }
    }

    // fn as_mmap(&mut self) -> Option<&mut memmap2::MmapMut> {
    //     Some(&mut self.mmap)
    // }

    fn filling(&mut self) -> usize {
        self.write_pos
    }

    fn capacity(&mut self) -> Option<usize> {
        Some(self.capacity)
    }

    fn kind(&self) -> &'static str {
        "circular-mmap"
    }
}

impl ViewableLogBuffer for CircularMappedMemFdBuffer {
    fn splice_from_and_view(
        &mut self,
        read_fd: RawFd,
        len: usize,
    ) -> std::io::Result<Option<&[u8]>> {
        let phys_off = self.write_pos % self.capacity;

        // clamp so a single splice never writes past the end of the ring; the
        // remainder (if any) is picked up on the caller's next call, now at offset 0
        let chunk = len.min(self.capacity - phys_off);

        let n = splice(
            read_fd,
            self.mmap.as_raw_fd(),
            Some(phys_off as libc::loff_t),
            chunk,
        )?;
        self.write_pos += n;
        match n {
            0 => Ok(None),
            _ => Ok(Some(&self.mmap[phys_off..phys_off + n])),
        }
    }

    fn read_from_cursor(&mut self, from: usize) -> LogChunk<'_> {
        let start = self.write_pos.saturating_sub(self.capacity);
        let (from, missed) = if from < start {
            let missed = start - from;
            log::warn!(
                "log buffer cursor {} fell behind retained window (start {}), data was overwritten before being flushed, missed {} bytes",
                from, start, missed
            );
            (start, missed)
        } else {
            (from, 0)
        };

        let len = self.write_pos - from;
        if len == 0 {
            return LogChunk::new(None, missed);
        }

        // clamp to the ring's wrap boundary; the caller re-invokes with the
        // advanced offset to pick up whatever's left on the other side
        let start_phys = from % self.capacity;
        let chunk_len = len.min(self.capacity - start_phys);
        let buf = &self.mmap[start_phys..start_phys + chunk_len];
        LogChunk::new(Some(buf), missed)
    }
}

impl AsFd for CircularMappedMemFdBuffer {
    fn as_fd(&self) -> std::os::fd::BorrowedFd<'_> {
        self.mmap.as_file().as_fd()
    }
}

impl AsRawFd for CircularMappedMemFdBuffer {
    fn as_raw_fd(&self) -> std::os::fd::RawFd {
        self.mmap.as_file().as_raw_fd()
    }
}

pub struct MemMapBuffer {
    mmap: MmapMut,
    file: File,
}

impl MemMapBuffer {
    pub fn new_from_file(file: File) -> std::io::Result<Self> {
        let mmap = unsafe { MmapMut::map_mut(&file)? };
        Ok(Self { mmap, file })
    }

    pub fn new_from_memfd(memfd_buffer: MemFdBuffer<Sealed>) -> std::io::Result<Self> {
        let file = memfd_buffer.into_file();
        Self::new_from_file(file)
    }
}

impl LogBuffer for MemMapBuffer {
    fn as_file(&self) -> &File {
        &self.file
    }

    fn filling(&mut self) -> usize {
        self.mmap.len()
    }

    fn capacity(&mut self) -> Option<usize> {
        Some(self.mmap.len())
    }

    fn kind(&self) -> &'static str {
        "mmap"
    }
}

impl ViewableLogBuffer for MemMapBuffer {
    fn splice_from_and_view(
        &mut self,
        read_fd: RawFd,
        len: usize,
    ) -> std::io::Result<Option<&[u8]>> {
        let chunk = len.min(self.mmap.len());
        let n = splice(read_fd, self.file.as_raw_fd(), Some(0), chunk)?;
        match n {
            0 => Ok(None),
            _ => Ok(Some(&self.mmap[0..n])),
        }
    }

    fn read_from_cursor(&mut self, from: usize) -> LogChunk<'_> {
        let from = from.min(self.mmap.len());
        let buf = &self.mmap[from..];
        LogChunk::new(Some(buf), 0)
    }
}

impl AsFd for MemMapBuffer {
    fn as_fd(&self) -> std::os::fd::BorrowedFd<'_> {
        self.file.as_fd()
    }
}
impl AsRawFd for MemMapBuffer {
    fn as_raw_fd(&self) -> std::os::fd::RawFd {
        self.file.as_raw_fd()
    }
}

pub struct DevNullLogBuffer(File);

impl DevNullLogBuffer {
    pub fn new() -> std::io::Result<Self> {
        let dev_null = CString::new("/dev/null").unwrap();
        let ret = unsafe { libc::open(dev_null.as_ptr(), libc::O_WRONLY) };
        let fd = to_ioresult(ret)?;
        let file = unsafe { File::from_raw_fd(fd) };
        Ok(Self(file))
    }
}

impl AsFd for DevNullLogBuffer {
    fn as_fd(&self) -> std::os::fd::BorrowedFd<'_> {
        self.0.as_fd()
    }
}

impl AsRawFd for DevNullLogBuffer {
    fn as_raw_fd(&self) -> std::os::fd::RawFd {
        self.0.as_raw_fd()
    }
}

impl LogBuffer for DevNullLogBuffer {
    fn as_file(&self) -> &File {
        &self.0
    }

    fn filling(&mut self) -> usize {
        0
    }

    fn capacity(&mut self) -> Option<usize> {
        None
    }

    fn kind(&self) -> &'static str {
        "dev-null"
    }
}
