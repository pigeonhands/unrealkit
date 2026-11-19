use crate::{Error, Result, Sdk, UeVersion};

/// AOB patterns used by SDK dumpers to locate UE engine globals.
///
/// These patterns match the instruction sequences that reference the global
/// pointers. After finding the pattern, we read the RIP-relative offset
/// from the matched instruction to compute the actual pointer address.
pub mod patterns {
    /// Common patterns for GNames (FNamePool / TNameEntryArray).
    ///
    /// UE5 GNames pattern — matches the LEA instruction that loads FNamePool:
    ///   `48 8D 05 ?? ?? ?? ??` followed by `EB` (LEA RAX, [RIP+disp32]; JMP)
    pub const GNAMES_UE5: &[Option<u8>] = &[
        Some(0x48), Some(0x8D), Some(0x05), None, None, None, None, // lea rax, [rip+??]
        Some(0xEB),                                                   // jmp
    ];

    /// UE4 GNames pattern — `48 8D 1D ?? ?? ?? ?? EB` (LEA RBX).
    pub const GNAMES_UE4: &[Option<u8>] = &[
        Some(0x48), Some(0x8D), Some(0x1D), None, None, None, None,
        Some(0xEB),
    ];

    /// Common patterns for GObjects (FUObjectArray).
    ///
    /// UE5 GObjects — `48 8B 05 ?? ?? ?? ?? 48 8B 0C C8 48 8D 04 D1`
    /// (MOV RAX, [RIP+disp]; MOV RCX, [RAX+RCX*8]; LEA RAX, [RCX+RDX*8])
    pub const GOBJECTS_UE5: &[Option<u8>] = &[
        Some(0x48), Some(0x8B), Some(0x05), None, None, None, None,
        Some(0x48), Some(0x8B), Some(0x0C), Some(0xC8),
        Some(0x48), Some(0x8D), Some(0x04), Some(0xD1),
    ];

    /// UE4 GObjects — `48 8B 05 ?? ?? ?? ?? 48 8B 0C C8 48 85 C9`
    pub const GOBJECTS_UE4: &[Option<u8>] = &[
        Some(0x48), Some(0x8B), Some(0x05), None, None, None, None,
        Some(0x48), Some(0x8B), Some(0x0C), Some(0xC8),
        Some(0x48), Some(0x85), Some(0xC9),
    ];

    /// GWorld pattern — `48 8B 05 ?? ?? ?? ?? 48 3B C3` (common in both UE4/5).
    pub const GWORLD: &[Option<u8>] = &[
        Some(0x48), Some(0x8B), Some(0x05), None, None, None, None,
        Some(0x48), Some(0x3B), Some(0xC3),
    ];

    /// GEngine pattern — `48 8B 0D ?? ?? ?? ?? 48 85 C9 74`
    pub const GENGINE: &[Option<u8>] = &[
        Some(0x48), Some(0x8B), Some(0x0D), None, None, None, None,
        Some(0x48), Some(0x85), Some(0xC9),
        Some(0x74),
    ];
}

/// Scan a memory region for one of the known patterns, then resolve the
/// RIP-relative offset to get the target pointer.
///
/// Returns the absolute address the instruction references.
///
/// # Safety
/// `base` and `size` must describe a readable memory region.
pub unsafe fn scan_rip_relative(
    base: *const u8,
    size: usize,
    pattern: &[Option<u8>],
) -> Option<*const u8> {
    let slice = unsafe { std::slice::from_raw_parts(base, size) };
    let offset = find_pattern(slice, pattern)?;

    // The RIP-relative displacement is at bytes [3..7] of the matched pattern
    // (the ?? bytes after the opcode). The instruction is 7 bytes long
    // (opcode + modrm + disp32), so RIP points past those 7 bytes.
    let disp_offset = offset + 3;
    let disp = i32::from_le_bytes([
        slice[disp_offset],
        slice[disp_offset + 1],
        slice[disp_offset + 2],
        slice[disp_offset + 3],
    ]);

    let instruction_end = unsafe { base.add(offset + 7) };
    let target = unsafe { instruction_end.offset(disp as isize) };
    Some(target)
}

/// Simple pattern matcher (wildcard = None).
fn find_pattern(haystack: &[u8], pattern: &[Option<u8>]) -> Option<usize> {
    if pattern.is_empty() || haystack.len() < pattern.len() {
        return None;
    }
    'outer: for i in 0..=(haystack.len() - pattern.len()) {
        for (j, p) in pattern.iter().enumerate() {
            if let Some(byte) = p {
                if haystack[i + j] != *byte {
                    continue 'outer;
                }
            }
        }
        return Some(i);
    }
    None
}

/// Attempt to automatically discover GNames, GObjects, and optionally GWorld
/// by scanning the game's .text section.
///
/// # Safety
/// `text_base` and `text_size` must describe the game's executable .text section.
pub unsafe fn discover(
    text_base: *const u8,
    text_size: usize,
    version: UeVersion,
) -> Result<Sdk> {
    let gnames_pattern = match version {
        UeVersion::Ue4 => patterns::GNAMES_UE4,
        UeVersion::Ue5 => patterns::GNAMES_UE5,
    };
    let gobjects_pattern = match version {
        UeVersion::Ue4 => patterns::GOBJECTS_UE4,
        UeVersion::Ue5 => patterns::GOBJECTS_UE5,
    };

    let gnames_ptr = unsafe { scan_rip_relative(text_base, text_size, gnames_pattern) }
        .ok_or(Error::InvalidGNames)?;

    let gobjects_ptr = unsafe { scan_rip_relative(text_base, text_size, gobjects_pattern) }
        .ok_or(Error::InvalidGObjects)?;

    unsafe { Sdk::from_pointers(gnames_ptr, gobjects_ptr, version) }
}

/// Scan for GWorld and return its pointer (the address of the global, which
/// holds a `*const UWorld`).
///
/// # Safety
/// `text_base` and `text_size` must describe a readable .text section.
pub unsafe fn find_gworld(text_base: *const u8, text_size: usize) -> Option<*const *const u8> {
    let ptr = unsafe { scan_rip_relative(text_base, text_size, patterns::GWORLD)? };
    Some(ptr as *const *const u8)
}

/// Scan for GEngine and return its pointer.
///
/// # Safety
/// `text_base` and `text_size` must describe a readable .text section.
pub unsafe fn find_gengine(text_base: *const u8, text_size: usize) -> Option<*const *const u8> {
    let ptr = unsafe { scan_rip_relative(text_base, text_size, patterns::GENGINE)? };
    Some(ptr as *const *const u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_pattern() {
        let haystack = [0x00, 0x48, 0x8D, 0x05, 0xAA, 0xBB, 0xCC, 0xDD, 0xEB, 0x00];
        let pattern = patterns::GNAMES_UE5;
        let result = find_pattern(&haystack, pattern);
        assert_eq!(result, Some(1));
    }

    #[test]
    fn test_find_pattern_no_match() {
        let haystack = [0x00, 0x00, 0x00, 0x00];
        let pattern = patterns::GNAMES_UE5;
        assert_eq!(find_pattern(&haystack, pattern), None);
    }
}
