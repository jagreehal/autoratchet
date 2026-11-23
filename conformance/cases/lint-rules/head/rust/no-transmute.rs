use std::mem::transmute_copy;

pub fn bits(x: f32) -> u32 {
    // SAFETY: f32 and u32 have the same size and every bit pattern is a valid u32.
    unsafe { std::mem::transmute(x) }
}

fn transmute(v: i32) -> i32 {
    v + 1
}

pub fn local(v: i32) -> i32 {
    transmute(v)
}

mod mem {
    pub fn transmute(v: i32) -> i32 {
        v
    }
}

pub fn project_mem(v: i32) -> i32 {
    mem::transmute(v)
}
