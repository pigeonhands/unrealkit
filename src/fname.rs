use crate::{Error, Result, UeVersion};

/// A resolved FName — an index into the global name table plus a number suffix.
#[derive(Debug, Clone, Copy)]
pub struct FName {
    /// Index into the name table (ComparisonIndex in UE source).
    pub index: u32,
    /// Instance number — 0 means no suffix, >0 appends `_N-1`.
    pub number: u32,
}

impl FName {
    /// Read an FName from memory at `ptr`.
    ///
    /// Layout: two consecutive u32 fields (index, number), 8 bytes total.
    ///
    /// # Safety
    /// `ptr` must point to a valid FName in process memory.
    pub unsafe fn read(ptr: *const u8) -> Self {
        let index = unsafe { (ptr as *const u32).read_unaligned() };
        let number = unsafe { (ptr.add(4) as *const u32).read_unaligned() };
        Self { index, number }
    }
}

/// Handle to the global name table (GNames).
///
/// UE4 uses a flat `TNameEntryArray` (array of pointers to FNameEntry).
/// UE5 uses `FNamePool` with a block-based allocator.
pub struct GNames {
    ptr: *const u8,
    version: UeVersion,
}

unsafe impl Send for GNames {}
unsafe impl Sync for GNames {}

impl GNames {
    /// Wrap a known GNames pointer.
    ///
    /// # Safety
    /// `ptr` must point to a valid GNames / FNamePool structure.
    pub unsafe fn from_ptr(ptr: *const u8, version: UeVersion) -> Result<Self> {
        if ptr.is_null() {
            return Err(Error::InvalidGNames);
        }
        Ok(Self { ptr, version })
    }

    /// Resolve an FName to its string representation.
    pub fn resolve(&self, name: FName) -> Result<String> {
        match self.version {
            UeVersion::Ue4 => unsafe { self.resolve_ue4(name) },
            UeVersion::Ue5 => unsafe { self.resolve_ue5(name) },
        }
    }

    /// UE4: TNameEntryArray — a TStaticIndirectArrayThreadSafeRead.
    ///
    /// Layout: array of chunk pointers, each chunk holds `ELEMENTS_PER_CHUNK`
    /// pointers to FNameEntry structs.
    ///
    /// ```text
    /// GNames -> [chunk0_ptr, chunk1_ptr, ...]
    ///            |
    ///            v
    ///           [entry0_ptr, entry1_ptr, ...] (ELEMENTS_PER_CHUNK entries)
    ///            |
    ///            v
    ///           FNameEntry { HashNext: i32, Index: i32, Chars: [u16] or [u8] }
    /// ```
    unsafe fn resolve_ue4(&self, name: FName) -> Result<String> {
        const ELEMENTS_PER_CHUNK: u32 = 16384; // 0x4000

        let chunk_index = name.index / ELEMENTS_PER_CHUNK;
        let within_chunk = name.index % ELEMENTS_PER_CHUNK;

        // GNames is a pointer to the array of chunk pointers.
        let chunks_base = self.ptr as *const *const *const u8;
        let chunk_ptr = unsafe { *chunks_base.add(chunk_index as usize) };
        if chunk_ptr.is_null() {
            return Err(Error::FNameOutOfRange(name.index));
        }

        let entry_ptr = unsafe { *chunk_ptr.add(within_chunk as usize) };
        if entry_ptr.is_null() {
            return Err(Error::FNameOutOfRange(name.index));
        }

        // FNameEntry layout (UE4):
        //   0x00: i32 — Index (encodes length and wide flag in lower bits)
        //   0x04: [u8...] or [u16...] — the name string
        //
        // The Index field's bit layout:
        //   bits [0..15]: name length
        //   bit 0 of the high half: wide flag
        //
        // Actually in UE4 the common layout is:
        //   0x00: i32 HashNext
        //   0x04: i32 Index  (FNameEntryId — not the same as the array index)
        //   0x08: union { AnsiName [u8], WideName [u16] }
        //
        // But the simpler "case header" approach (post-4.23) uses:
        //   0x00: u16 header — bits[0]: bIsWide, bits[1..15]: Len
        //   0x02: [u8; Len] or [u16; Len]
        //
        // We try the post-4.23 header layout first (more common in games
        // people are actually modding).

        // Skip the hash field (2 bytes in post-4.23 entries, stored before header).
        // Post-4.23 FNameEntry layout:
        //   0x00: u16 — header (bit 0 = bIsWide, bits 1..15 = len)
        //   0x02: chars

        let header = unsafe { (entry_ptr as *const u16).read_unaligned() };
        let is_wide = (header & 1) != 0;
        let len = (header >> 1) as usize;

        if is_wide {
            let chars_ptr = unsafe { entry_ptr.add(2) as *const u16 };
            let slice = unsafe { std::slice::from_raw_parts(chars_ptr, len) };
            let s = String::from_utf16_lossy(slice);
            Ok(format_fname(s, name.number))
        } else {
            let chars_ptr = unsafe { entry_ptr.add(2) };
            let slice = unsafe { std::slice::from_raw_parts(chars_ptr, len) };
            let s = String::from_utf8_lossy(slice).into_owned();
            Ok(format_fname(s, name.number))
        }
    }

    /// UE5: FNamePool — block-based allocator.
    ///
    /// ```text
    /// FNamePool:
    ///   0x00: FNamePoolShard Lock  (pad/skip)
    ///   0x08: u32 CurrentBlock
    ///   0x0C: u32 CurrentByteCursor
    ///   0x10: [*const u8; MaxBlocks] Blocks
    ///
    /// An FNameEntry is at:
    ///   Block = Blocks[index >> BLOCK_OFFSET_BITS]
    ///   Offset = (index & BLOCK_OFFSET_MASK) * STRIDE
    ///   Entry = Block + Offset
    ///
    /// FNameEntry (UE5):
    ///   0x00: u16 Header — bit 0 = bIsWide, bits 1..15 = Len
    ///   0x02: [u8; Len] or [u16; Len]
    /// ```
    unsafe fn resolve_ue5(&self, name: FName) -> Result<String> {
        // FNamePool layout in UE5:
        // The global GNames in UE5 is a FNamePool.
        // Entries are accessed via FNameEntryId which encodes block + offset.

        // FNameEntryId bit layout:
        //   bits [0..BLOCK_OFFSET_BITS): offset within block (in strides of 2)
        //   bits [BLOCK_OFFSET_BITS..): block index
        const FNAME_BLOCK_OFFSET_BITS: u32 = 16;
        const FNAME_BLOCK_OFFSET_MASK: u32 = (1 << FNAME_BLOCK_OFFSET_BITS) - 1;
        const STRIDE: usize = 2; // entries are 2-byte aligned

        let block_idx = (name.index >> FNAME_BLOCK_OFFSET_BITS) as usize;
        let offset = (name.index & FNAME_BLOCK_OFFSET_MASK) as usize * STRIDE;

        // FNamePool::Entries is at offset 0x10 from the pool base (after the
        // lock + CurrentBlock + CurrentByteCursor).
        // But the actual layout depends on the FNamePoolShardBase size.
        // In practice, the Blocks array is at a fixed offset from the pool.
        //
        // Common layout (from Dumper-7):
        //   0x00: FRWLock (8 bytes on x64)
        //   0x08: u32 CurrentBlock
        //   0x0C: u32 CurrentByteCursor
        //   0x10: [*const u8; MAX_BLOCKS] Blocks
        //
        // But GNames in UE5 is actually an array of FNamePool shards, not
        // a single pool. The entry ID includes a shard index as well.
        //
        // Simplified (single-shard for now — most implementations treat it
        // as one flat pool since the shard is encoded in the high bits of
        // the ComparisonIndex):

        const ENTRIES_OFFSET: usize = 0x10;

        let blocks_base = unsafe { self.ptr.add(ENTRIES_OFFSET) as *const *const u8 };
        let block_ptr = unsafe { *blocks_base.add(block_idx) };
        if block_ptr.is_null() {
            return Err(Error::FNameOutOfRange(name.index));
        }

        let entry_ptr = unsafe { block_ptr.add(offset) };

        let header = unsafe { (entry_ptr as *const u16).read_unaligned() };
        let is_wide = (header & 1) != 0;
        let len = (header >> 1) as usize;

        if is_wide {
            let chars_ptr = unsafe { entry_ptr.add(2) as *const u16 };
            let slice = unsafe { std::slice::from_raw_parts(chars_ptr, len) };
            let s = String::from_utf16_lossy(slice);
            Ok(format_fname(s, name.number))
        } else {
            let chars_ptr = unsafe { entry_ptr.add(2) };
            let slice = unsafe { std::slice::from_raw_parts(chars_ptr, len) };
            let s = String::from_utf8_lossy(slice).into_owned();
            Ok(format_fname(s, name.number))
        }
    }

    pub fn ptr(&self) -> *const u8 {
        self.ptr
    }
}

fn format_fname(base: String, number: u32) -> String {
    if number == 0 {
        base
    } else {
        format!("{}_{}", base, number - 1)
    }
}
