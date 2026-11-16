#![allow(clippy::cognitive_complexity)]
use std::fmt;

pub struct Wrapper<T>(pub T);

impl<T: fmt::Display> fmt::Display for Wrapper<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if f.alternate() { write!(f, "[{}]", self.0) } else { write!(f, "{}", self.0) }
    }
}

impl<T> Wrapper<T> {
    pub fn new(value: T) -> Self { Self(value) }
    pub fn map<U>(self, f: impl Fn(T) -> U) -> Wrapper<U> { Wrapper(f(self.0)) }
}

pub trait Shape {
    fn area(&self) -> f64;
    fn describe(&self) -> String {
        if self.area() > 10.0 { "large".into() } else { "small".into() }
    }
}

pub mod billing {
    pub mod tax {
        pub fn rate(country: &str) -> u8 { if country == "GB" { 20 } else { 0 } }
    }
    pub fn charge(amounts: &[u32]) -> u32 {
        fn round(x: u32) -> u32 { x / 10 * 10 }
        let on_error = |code: u8| if code > 1 { 0 } else { 1 };
        let total = amounts.iter().map(|a| round(*a)).filter(|a| *a > 0).sum::<u32>();
        total + u32::from(on_error(0))
    }
}

pub fn twice() -> u8 { 1 }

pub mod again {
    pub fn twice() -> u8 { 2 }
}

pub fn callbacks(v: Vec<u8>) -> usize {
    let a = v.iter().filter(|x| **x > 1).count();
    let b = v.iter().filter(|x| **x > 2).count();
    std::thread::spawn(move || a + b).join().unwrap_or(0)
}

#[cfg(test)]
mod tests {
    #[test]
    fn it_works() { assert_eq!(super::twice(), 1); }
}
