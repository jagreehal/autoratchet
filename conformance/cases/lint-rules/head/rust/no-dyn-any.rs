use std::any::Any;

pub fn size(value: Box<dyn Any>) -> usize {
    std::mem::size_of_val(&value)
}

pub fn panic_message(panic: &(dyn std::any::Any + Send)) -> String {
    format!("{panic:?}")
}

pub fn shared(value: &(dyn std::any::Any + Send + Sync)) -> usize {
    std::mem::size_of_val(value)
}
