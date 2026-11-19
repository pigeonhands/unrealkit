use crate::fname::{FName, GNames};
use crate::UeVersion;

/// A live UObject in the target process's memory.
///
/// UObjectBase layout (common to UE4 and UE5):
/// ```text
///   0x00: *const () — vtable pointer
///   0x08: i32       — ObjectFlags (EObjectFlags)
///   0x0C: i32       — InternalIndex (index in GObjects)
///   0x10: *const UClass — ClassPrivate
///   0x18: FName     — NamePrivate (u32 index + u32 number = 8 bytes)
///   0x20: *const UObject — OuterPrivate
/// ```
/// Total UObjectBase size: 0x28 (40 bytes).
pub struct UObject<'a> {
    ptr: *const u8,
    names: &'a GNames,
}

impl<'a> UObject<'a> {
    pub fn new(ptr: *const u8, names: &'a GNames) -> Self {
        Self { ptr, names }
    }

    pub fn ptr(&self) -> *const u8 {
        self.ptr
    }

    /// The object's flags.
    pub fn flags(&self) -> i32 {
        unsafe { *(self.ptr.add(0x08) as *const i32) }
    }

    /// The object's index in the GObjects array.
    pub fn internal_index(&self) -> i32 {
        unsafe { *(self.ptr.add(0x0C) as *const i32) }
    }

    /// Raw pointer to this object's UClass.
    fn class_ptr(&self) -> *const u8 {
        unsafe { *(self.ptr.add(0x10) as *const *const u8) }
    }

    /// The object's UClass.
    pub fn class(&self) -> UClass<'a> {
        UClass::new(self.class_ptr(), self.names)
    }

    /// The raw FName for this object.
    pub fn fname(&self) -> FName {
        unsafe { FName::read(self.ptr.add(0x18)) }
    }

    /// The object's short name (e.g. "PlayerController_0").
    pub fn name(&self) -> String {
        self.names.resolve(self.fname()).unwrap_or_else(|_| "<invalid>".to_string())
    }

    /// Raw pointer to the outer object (package hierarchy parent).
    fn outer_ptr(&self) -> *const u8 {
        unsafe { *(self.ptr.add(0x20) as *const *const u8) }
    }

    /// The outer object, if any.
    pub fn outer(&self) -> Option<UObject<'a>> {
        let ptr = self.outer_ptr();
        if ptr.is_null() {
            None
        } else {
            Some(UObject::new(ptr, self.names))
        }
    }

    /// Build the full path name: "ClassName Outer.Outer.Name".
    pub fn full_name(&self) -> String {
        let class_name = self.class().name();
        let mut path_parts = vec![self.name()];
        let mut current = self.outer();
        while let Some(outer) = current {
            path_parts.push(outer.name());
            current = outer.outer();
        }
        path_parts.reverse();
        let path = path_parts.join(".");
        format!("{} {}", class_name, path)
    }

    /// Check if this object is an instance of a given class (by name).
    pub fn is_a(&self, class_name: &str) -> bool {
        let mut cls = Some(self.class());
        while let Some(c) = cls {
            if c.name() == class_name {
                return true;
            }
            cls = c.super_class();
        }
        false
    }
}

/// A UStruct — the base for UClass, UScriptStruct, and UFunction.
///
/// Offsets differ between UE4.19 and UE4.25+/UE5 due to added FField support:
///
/// ```text
/// UE4.19 (inherits UField @ 0x30):
///   0x30: SuperStruct    0x38: Children    0x40: PropertiesSize
///
/// UE4.25+ / UE5 (inherits UField @ 0x30, +padding):
///   0x40: SuperStruct    0x48: Children    0x50: ChildProperties    0x58: PropertiesSize
/// ```
pub struct UStruct<'a> {
    ptr: *const u8,
    names: &'a GNames,
}

impl<'a> UStruct<'a> {
    pub fn new(ptr: *const u8, names: &'a GNames) -> Self {
        Self { ptr, names }
    }

    pub fn ptr(&self) -> *const u8 {
        self.ptr
    }

    fn version(&self) -> UeVersion {
        self.names.version()
    }

    pub fn as_object(&self) -> UObject<'a> {
        UObject::new(self.ptr, self.names)
    }

    pub fn name(&self) -> String {
        self.as_object().name()
    }

    fn super_struct_offset(&self) -> usize {
        match self.version() {
            UeVersion::Ue4 => 0x30,
            UeVersion::Ue5 => 0x40,
        }
    }

    fn children_offset(&self) -> usize {
        match self.version() {
            UeVersion::Ue4 => 0x38,
            UeVersion::Ue5 => 0x48,
        }
    }

    fn properties_size_offset(&self) -> usize {
        match self.version() {
            UeVersion::Ue4 => 0x40,
            UeVersion::Ue5 => 0x58,
        }
    }

    pub fn super_struct(&self) -> Option<UStruct<'a>> {
        let ptr = unsafe { *(self.ptr.add(self.super_struct_offset()) as *const *const u8) };
        if ptr.is_null() {
            None
        } else {
            Some(UStruct::new(ptr, self.names))
        }
    }

    pub fn children_ptr(&self) -> *const u8 {
        unsafe { *(self.ptr.add(self.children_offset()) as *const *const u8) }
    }

    pub fn properties_size(&self) -> i32 {
        unsafe { *(self.ptr.add(self.properties_size_offset()) as *const i32) }
    }

    pub fn children(&self) -> UFieldIter<'a> {
        UFieldIter {
            current: self.children_ptr(),
            names: self.names,
        }
    }
}

/// A UClass — extends UStruct with class-specific data.
///
/// UClass adds:
/// ```text
///   ... UStruct fields ...
///   ClassDefaultObject: *const UObject
///   ... many more fields ...
/// ```
pub struct UClass<'a> {
    ptr: *const u8,
    names: &'a GNames,
}

impl<'a> UClass<'a> {
    pub fn new(ptr: *const u8, names: &'a GNames) -> Self {
        Self { ptr, names }
    }

    pub fn ptr(&self) -> *const u8 {
        self.ptr
    }

    pub fn as_struct(&self) -> UStruct<'a> {
        UStruct::new(self.ptr, self.names)
    }

    pub fn as_object(&self) -> UObject<'a> {
        UObject::new(self.ptr, self.names)
    }

    pub fn name(&self) -> String {
        self.as_object().name()
    }

    /// The parent class (follows SuperStruct).
    pub fn super_class(&self) -> Option<UClass<'a>> {
        self.as_struct().super_struct().map(|s| UClass::new(s.ptr(), self.names))
    }

    /// Iterate the inheritance chain (self, super, super's super, ...).
    pub fn class_hierarchy(&self) -> ClassHierarchyIter<'a> {
        ClassHierarchyIter {
            current: Some(UClass::new(self.ptr, self.names)),
        }
    }

    /// Iterate UField children of this class.
    pub fn children(&self) -> UFieldIter<'a> {
        self.as_struct().children()
    }

    /// Size of an instance of this class in bytes.
    pub fn instance_size(&self) -> i32 {
        self.as_struct().properties_size()
    }
}

/// A UField — base class for properties and functions in the reflection chain.
///
/// Each UField has a Next pointer forming a linked list within the parent struct.
pub struct UField<'a> {
    ptr: *const u8,
    names: &'a GNames,
}

impl<'a> UField<'a> {
    pub fn ptr(&self) -> *const u8 {
        self.ptr
    }

    pub fn as_object(&self) -> UObject<'a> {
        UObject::new(self.ptr, self.names)
    }

    pub fn name(&self) -> String {
        self.as_object().name()
    }

    pub fn class_name(&self) -> String {
        self.as_object().class().name()
    }

    /// Next field in the linked list.
    fn next_ptr(&self) -> *const u8 {
        // UField::Next is at offset 0x28 (right after UObject).
        unsafe { *(self.ptr.add(0x28) as *const *const u8) }
    }
}

/// Iterator over a UField linked list.
pub struct UFieldIter<'a> {
    current: *const u8,
    names: &'a GNames,
}

impl<'a> Iterator for UFieldIter<'a> {
    type Item = UField<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.current.is_null() {
            return None;
        }
        let field = UField {
            ptr: self.current,
            names: self.names,
        };
        self.current = field.next_ptr();
        Some(field)
    }
}

/// Iterator walking up the class inheritance chain.
pub struct ClassHierarchyIter<'a> {
    current: Option<UClass<'a>>,
}

impl<'a> Iterator for ClassHierarchyIter<'a> {
    type Item = UClass<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        let cls = self.current.take()?;
        self.current = cls.super_class();
        Some(cls)
    }
}
