pub mod discovery;
mod error;
mod fname;
mod gobjects;
mod uobject;
mod version;

pub use error::Error;
pub use fname::{FName, GNames};
pub use gobjects::GObjects;
pub use uobject::{UClass, UField, UObject, UStruct};
pub use version::UeVersion;

pub type Result<T> = std::result::Result<T, Error>;

/// Top-level handle to a running Unreal Engine game's internals.
///
/// Created by discovering GNames and GObjects in the process memory,
/// either via AOB pattern scanning or known offsets.
pub struct Sdk {
    pub names: GNames,
    pub objects: GObjects,
}

impl Sdk {
    /// Create from already-known pointers.
    ///
    /// # Safety
    /// `gnames` and `gobjects` must point to valid UE GNames and GObjects
    /// structures in the current process.
    pub unsafe fn from_pointers(gnames: *const u8, gobjects: *const u8, version: UeVersion) -> Result<Self> {
        let names = unsafe { GNames::from_ptr(gnames, version)? };
        let objects = unsafe { GObjects::from_ptr(gobjects, version)? };
        Ok(Self { names, objects })
    }

    /// Iterate all live UObjects.
    pub fn objects(&self) -> impl Iterator<Item = UObject<'_>> {
        self.objects.iter(&self.names)
    }

    /// Find the first object whose full name matches a predicate.
    pub fn find_object<F>(&self, predicate: F) -> Option<UObject<'_>>
    where
        F: Fn(&str) -> bool,
    {
        self.objects().find(|obj| predicate(&obj.full_name()))
    }

    /// Find the first object with the given name (short name, not full path).
    pub fn find_object_by_name(&self, name: &str) -> Option<UObject<'_>> {
        self.objects().find(|obj| obj.name() == name)
    }
}
