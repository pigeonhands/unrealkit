/// Which major Unreal Engine generation we're targeting.
///
/// The GNames and GObjects layouts differ significantly between UE4 and UE5.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UeVersion {
    /// UE 4.x — TNameEntryArray (flat pointer array) and flat FUObjectArray.
    Ue4,
    /// UE 5.x — FNamePool (block-based) and chunked FUObjectArray.
    Ue5,
}
