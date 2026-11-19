use crate::{Error, GNames, Result, UObject, UeVersion};

// ---------------------------------------------------------------------------
// Raw UE structs — #[repr(C)] so the compiler matches engine layout exactly.
// ---------------------------------------------------------------------------

/// A single slot in the GObjects array (identical across UE 4.19 – 5.x).
///
/// 0x18 bytes (24).
#[repr(C)]
pub struct FUObjectItem {
    pub object: *const u8,
    pub flags: i32,
    pub cluster_root_index: i32,
    pub serial_number: i32,
    _pad: i32,
}

/// UE4 flat object array — `TArray<FUObjectItem>` embedded in FUObjectArray.
///
/// ```text
/// +0x00  *FUObjectItem  objects
/// +0x08  i32            max_elements
/// +0x0C  i32            num_elements
/// ```
#[repr(C)]
pub struct FFixedUObjectArray {
    pub objects: *const FUObjectItem,
    pub max_elements: i32,
    pub num_elements: i32,
}

/// UE5 chunked object array — `FChunkedFixedUObjectArray`.
///
/// ```text
/// +0x00  **FUObjectItem  objects          (array of chunk pointers)
/// +0x08  *FUObjectItem   pre_allocated
/// +0x10  i32             max_elements
/// +0x14  i32             num_elements
/// +0x18  i32             max_chunks
/// +0x1C  i32             num_chunks
/// ```
#[repr(C)]
pub struct FChunkedFixedUObjectArray {
    pub objects: *const *const FUObjectItem,
    pub pre_allocated: *const FUObjectItem,
    pub max_elements: i32,
    pub num_elements: i32,
    pub max_chunks: i32,
    pub num_chunks: i32,
}

/// Header shared by both UE4 and UE5 FUObjectArray, before the inner array.
///
/// ```text
/// +0x00  i32   obj_first_gc_index
/// +0x04  i32   obj_last_non_gc_index
/// +0x08  i32   max_objects_not_considered_by_gc
/// +0x0C  bool  open_for_disregard_for_gc
/// +0x10  ...   (inner array starts here)
/// ```
const INNER_ARRAY_OFFSET: usize = 0x10;

// ---------------------------------------------------------------------------
// High-level wrapper
// ---------------------------------------------------------------------------

/// Handle to the global UObject array (GObjects / GUObjectArray).
pub struct GObjects {
    ptr: *const u8,
    version: UeVersion,
}

unsafe impl Send for GObjects {}
unsafe impl Sync for GObjects {}

impl GObjects {
    /// Wrap a known GObjects pointer.
    ///
    /// # Safety
    /// `ptr` must point to a valid FUObjectArray / GUObjectArray.
    pub unsafe fn from_ptr(ptr: *const u8, version: UeVersion) -> Result<Self> {
        if ptr.is_null() {
            return Err(Error::InvalidGObjects);
        }
        Ok(Self { ptr, version })
    }

    /// Pointer to the inner array struct (at +0x10 from the FUObjectArray base).
    fn inner_ptr(&self) -> *const u8 {
        unsafe { self.ptr.add(INNER_ARRAY_OFFSET) }
    }

    fn as_fixed(&self) -> &FFixedUObjectArray {
        unsafe { &*(self.inner_ptr() as *const FFixedUObjectArray) }
    }

    fn as_chunked(&self) -> &FChunkedFixedUObjectArray {
        unsafe { &*(self.inner_ptr() as *const FChunkedFixedUObjectArray) }
    }

    pub fn num_elements(&self) -> i32 {
        match self.version {
            UeVersion::Ue4 => self.as_fixed().num_elements,
            UeVersion::Ue5 => self.as_chunked().num_elements,
        }
    }

    pub fn max_elements(&self) -> i32 {
        match self.version {
            UeVersion::Ue4 => self.as_fixed().max_elements,
            UeVersion::Ue5 => self.as_chunked().max_elements,
        }
    }

    /// Read the UObject pointer at the given index.
    unsafe fn get_object_ptr(&self, index: i32) -> *const u8 {
        match self.version {
            UeVersion::Ue4 => {
                let arr = self.as_fixed();
                let item = unsafe { &*arr.objects.add(index as usize) };
                item.object
            }
            UeVersion::Ue5 => {
                const ELEMENTS_PER_CHUNK: i32 = 64 * 1024;

                let arr = self.as_chunked();
                let chunk_index = index / ELEMENTS_PER_CHUNK;
                let within_chunk = index % ELEMENTS_PER_CHUNK;

                let chunk = unsafe { *arr.objects.add(chunk_index as usize) };
                if chunk.is_null() {
                    return std::ptr::null();
                }
                let item = unsafe { &*chunk.add(within_chunk as usize) };
                item.object
            }
        }
    }

    /// Iterate all live UObjects.
    pub fn iter<'a>(&'a self, names: &'a GNames) -> GObjectsIter<'a> {
        GObjectsIter {
            objects: self,
            names,
            index: 0,
            count: self.num_elements(),
        }
    }

    pub fn ptr(&self) -> *const u8 {
        self.ptr
    }
}

pub struct GObjectsIter<'a> {
    objects: &'a GObjects,
    names: &'a GNames,
    index: i32,
    count: i32,
}

impl<'a> Iterator for GObjectsIter<'a> {
    type Item = UObject<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        while self.index < self.count {
            let idx = self.index;
            self.index += 1;

            let ptr = unsafe { self.objects.get_object_ptr(idx) };
            if !ptr.is_null() {
                return Some(UObject::new(ptr, self.names));
            }
        }
        None
    }
}
