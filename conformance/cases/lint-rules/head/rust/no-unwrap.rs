pub fn first(xs: &[u8]) -> u8 {
    *xs.first().unwrap()
}

pub fn first_or(xs: &[u8]) -> u8 {
    *xs.first().expect("callers pass a non-empty slice")
}

#[cfg(test)]
mod tests {
    #[test]
    fn first_works() {
        assert_eq!(super::first(&[1]), "1".parse::<u8>().unwrap());
    }
}

#[cfg(not(test))]
pub fn production_only(xs: &[u8]) -> u8 {
    *xs.first().unwrap()
}

#[cfg(any(test, unix))]
pub fn also_production(xs: &[u8]) -> u8 {
    *xs.first().unwrap()
}

#[cfg(all(test, unix))]
pub fn test_only(xs: &[u8]) -> u8 {
    *xs.first().unwrap()
}

pub fn locked(m: &std::sync::Mutex<u8>, l: &std::sync::RwLock<u8>) -> u8 {
    *m.lock().unwrap() + *l.read().expect("no writer panics") + *l.write().expect("no writer panics")
}
