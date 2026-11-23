pub fn read(p: *const u8) -> u8 {
    unsafe { *p }
}

pub fn read_checked(p: *const u8) -> u8 {
    // SAFETY: callers pass a pointer from a live slice.
    unsafe { *p }
}
