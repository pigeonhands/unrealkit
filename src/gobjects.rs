use crate::{Error, GNames, Result, UObject, UeVersion};

/// Handle to the global UObject array (GObjects / GUObjectArray).
///
/// UE4: flat `TUObjectArray` — a simple `TArray<FUObjectItem>`.
/// UE5: chunked — an array of fixed-size chunk pointers.
pub struct GObjects {
    ptr: *const u8,
    version: UeVersion,
}

unsafe impl Send for GObjects {}
unsafe impl Sync for GObjects {}

/// A single entry in the GObjects array.
#[repr(C)]
struct FUObjectItem {
    /// Pointer to the UObject.
    object: *const u8,
    /// Internal flags (not important for reading).
    flags: i32,
    /// Cluster root index.
    cluster_root_index: i32,
    /// Serial number for weak references.
    serial_number: i32,
    _pad: i32,
}

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

    /// Get the total number of elements (including null/dead slots).
    fn num_elements(&self) -> i32 {
        match self.version {
            UeVersion::Ue4 => {
                // FUObjectArray layout (UE4):
                //   0x00: i32 ObjFirstGCIndex
                //   0x04: i32 ObjLastNonGCIndex
                //   0x08: i32 MaxObjectsNotConsideredByGC
                //   0x0C: bool OpenForDisregardForGC
                //   0x10: TArray<FUObjectItem> ObjObjects
                //     0x10: *const FUObjectItem Data
                //     0x18: i32 NumElements
                //     0x1C: i32 MaxElements
                //
                // NumElements is at offset 0x18 from the base.
                unsafe { *(self.ptr.add(0x18) as *const i32) }
            }
            UeVersion::Ue5 => {
                // FUObjectArray layout (UE5 chunked):
                //   0x00: i32 ObjFirstGCIndex
                //   0x04: i32 ObjLastNonGCIndex
                //   0x08: i32 MaxObjectsNotConsideredByGC
                //   0x0C: bool OpenForDisregardForGC
                //   0x10: FChunkedFixedUObjectArray ObjObjects
                //     0x10: **FUObjectItem Objects (array of chunk ptrs)
                //     0x18: *FUObjectItem PreAllocatedObjects
                //     0x20: i32 MaxElements
                //     0x24: i32 NumElements
                //     0x28: i32 MaxChunks
                //     0x2C: i32 NumChunks
                //
                // NumElements is at offset 0x24.
                unsafe { *(self.ptr.add(0x24) as *const i32) }
            }
        }
    }

    /// Read the UObject pointer at the given index.
    unsafe fn get_object_ptr(&self, index: i32) -> *const u8 {
        match self.version {
            UeVersion::Ue4 => {
                // Flat array: Data is at offset 0x10.
                let data = unsafe { *(self.ptr.add(0x10) as *const *const FUObjectItem) };
                let item = unsafe { &*data.add(index as usize) };
                item.object
            }
            UeVersion::Ue5 => {
                // Chunked: each chunk holds ELEMENTS_PER_CHUNK items.
                const ELEMENTS_PER_CHUNK: i32 = 64 * 1024; // 65536

                let chunk_index = index / ELEMENTS_PER_CHUNK;
                let within_chunk = index % ELEMENTS_PER_CHUNK;

                // Chunks array pointer is at offset 0x10.
                let chunks = unsafe { *(self.ptr.add(0x10) as *const *const *const FUObjectItem) };
                let chunk = unsafe { *chunks.add(chunk_index as usize) };
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
