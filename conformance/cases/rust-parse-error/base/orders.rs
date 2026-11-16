pub struct Order {
    pub items: Vec<u32>,
    pub vip: bool,
}

impl Order {
    pub fn total(&self) -> u32 {
        self.items.iter().sum()
    }

    pub fn discount(&self) -> u32 {
        if self.vip { 10 } else { 0 }
    }
}

pub fn route(order: &Order, express: bool) -> &'static str {
    if express {
        return "express";
    }
    if order.vip {
        return "vip";
    }
    "standard"
}
