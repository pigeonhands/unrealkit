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

    /// UE4 (pre-4.23): TNameEntryArray — TStaticIndirectArrayThreadSafeRead.
    ///
    /// ```text
    /// GNames -> [chunk0_ptr, chunk1_ptr, ...]  (256 chunks max)
    ///            |
    ///            v
    ///           [entry0_ptr, entry1_ptr, ...]   (0x4000 per chunk)
    ///            |
    ///            v
    ///           FNameEntry:
    ///             0x00: i32 (bit 0 = is_wide, bits 1..31 = index)
    ///             0x04: i32 (padding / hash)
    ///             0x08: *FNameEntry HashNext (8 bytes on x64)
    ///             0x10: char[] or wchar_t[] (null-terminated)
    /// ```
    unsafe fn resolve_ue4(&self, name: FName) -> Result<String> {
        const ELEMENTS_PER_CHUNK: u32 = 0x4000;

        let chunk_index = name.index / ELEMENTS_PER_CHUNK;
        let within_chunk = name.index % ELEMENTS_PER_CHUNK;

        let chunks_base = self.ptr as *const *const *const u8;
        let chunk_ptr = unsafe { *chunks_base.add(chunk_index as usize) };
        if chunk_ptr.is_null() {
            return Err(Error::FNameOutOfRange(name.index));
        }

        let entry_ptr = unsafe { *chunk_ptr.add(within_chunk as usize) };
        if entry_ptr.is_null() {
            return Err(Error::FNameOutOfRange(name.index));
        }

        // Pre-4.23: header i32 at 0x00, bit 0 = is_wide. String at 0x10 (past
        // the HashNext pointer), null-terminated.
        let header = unsafe { (entry_ptr as *const i32).read_unaligned() };
        let is_wide = (header & 1) != 0;

        const STRING_OFFSET: usize = 0x10;
        let chars_ptr = unsafe { entry_ptr.add(STRING_OFFSET) };

        let s = if is_wide {
            let mut len = 0usize;
            let wide_ptr = chars_ptr as *const u16;
            while unsafe { *wide_ptr.add(len) } != 0 {
                len += 1;
            }
            let slice = unsafe { std::slice::from_raw_parts(wide_ptr, len) };
            String::from_utf16_lossy(slice)
        } else {
            let mut len = 0usize;
            while unsafe { *chars_ptr.add(len) } != 0 {
                len += 1;
            }
            let slice = unsafe { std::slice::from_raw_parts(chars_ptr, len) };
            String::from_utf8_lossy(slice).into_owned()
        };
        Ok(format_fname(s, name.number))
    }

    /// UE5 (and UE 4.23+): FNamePool — block-based allocator.
    ///
    /// ```text
    /// FNamePool:
    ///   0x00: FRWLock (8 bytes)
    ///   0x08: u32 CurrentBlock
    ///   0x0C: u32 CurrentByteCursor
    ///   0x10: [*const u8; MaxBlocks] Blocks
    ///
    /// FNameEntryId encoding:
    ///   block_index  = comparison_index >> 16
    ///   byte_offset  = (comparison_index & 0xFFFF) * 2
    ///
    /// FNameEntry (UE5):
    ///   0x00: u16 Header — bit 0 = bIsWide, bits 1-5 = probe hash, bits 6-15 = Len
    ///   0x02: [u8; Len] or [u16; Len]
    /// ```
    unsafe fn resolve_ue5(&self, name: FName) -> Result<String> {
        const FNAME_BLOCK_OFFSET_BITS: u32 = 16;
        const FNAME_BLOCK_OFFSET_MASK: u32 = (1 << FNAME_BLOCK_OFFSET_BITS) - 1;
        const STRIDE: usize = 2;
        const BLOCKS_OFFSET: usize = 0x10;

        let block_idx = (name.index >> FNAME_BLOCK_OFFSET_BITS) as usize;
        let offset = (name.index & FNAME_BLOCK_OFFSET_MASK) as usize * STRIDE;

        let blocks_base = unsafe { self.ptr.add(BLOCKS_OFFSET) as *const *const u8 };
        let block_ptr = unsafe { *blocks_base.add(block_idx) };
        if block_ptr.is_null() {
            return Err(Error::FNameOutOfRange(name.index));
        }

        let entry_ptr = unsafe { block_ptr.add(offset) };

        // Header: bit 0 = bIsWide, bits 1-5 = probe hash (unused), bits 6-15 = length.
        let header = unsafe { (entry_ptr as *const u16).read_unaligned() };
        let is_wide = (header & 1) != 0;
        let len = (header >> 6) as usize;

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

    pub fn version(&self) -> UeVersion {
        self.version
    }
}

fn format_fname(base: String, number: u32) -> String {
    if number == 0 {
        base
    } else {
        format!("{}_{}", base, number - 1)
    }
}
