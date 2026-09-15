pub struct MioTokenSlab {
    slab: slab::Slab<mio::Token>,
    /// Offset to be added to the token value to avoid low-numbered tokens (statically reserved).
    offset: usize,
}

impl MioTokenSlab {
    pub fn new(init_capacity: usize, offset: usize) -> Self {
        MioTokenSlab {
            slab: slab::Slab::with_capacity(init_capacity),
            offset,
        }
    }

    pub fn allocate(&mut self) -> Option<mio::Token> {
        if self.slab.len() >= self.slab.capacity() {
            log::warn!("MioTokenSlab capacity reached, cannot allocate new token");
            return None;
        }

        let entry = self.slab.vacant_entry();
        let key = entry.key();
        let token = mio::Token(self.offset + key);
        entry.insert(token);
        log::debug!("Allocated new token: {:?}", token);
        Some(token)
    }

    pub fn free(&mut self, token: mio::Token) {
        let key = token.0 - self.offset;
        self.slab.remove(key);
        log::debug!("Freed token: {:?}", token);
    }
}
