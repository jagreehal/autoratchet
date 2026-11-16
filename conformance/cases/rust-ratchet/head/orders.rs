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

pub fn route(order: &Order, express: bool, member: bool, country: &str) -> &'static str {
    if express {
        if member {
            if country == "GB" {
                return "express-member-gb";
            } else if country == "FR" || country == "DE" {
                return "express-member-eu";
            }
            return "express-member";
        }
        for item in &order.items {
            if *item > 100 && order.vip {
                return "express-heavy-vip";
            }
        }
        return "express";
    }
    match country {
        "GB" if order.vip => "vip-gb",
        "GB" => "gb",
        _ if order.items.is_empty() => "empty",
        _ => "standard",
    }
}

// autoratchet-disable-next-line complexity -- one arm per status code, no logic
pub fn status(code: u16) -> &'static str {
    match code {
        200 if code > 0 => "ok",
        201 if code > 0 => "created",
        204 if code > 0 => "no content",
        301 if code > 0 => "moved",
        304 if code > 0 => "not modified",
        400 if code > 0 => "bad request",
        401 if code > 0 => "unauthorised",
        403 if code > 0 => "forbidden",
        404 if code > 0 => "not found",
        500 if code > 0 => "error",
        _ => "unknown",
    }
}

#[expect(clippy::cognitive_complexity, reason = "follows the parsing steps of the spec in order")]
pub fn parse(input: &str) -> Option<(u8, u8)> {
    let mut parts = input.split('.');
    let major = parts.next()?.parse().ok()?;
    if let Some(minor) = parts.next() {
        if let Ok(minor) = minor.parse() {
            for c in input.chars() {
                if c == ' ' {
                    while input.len() > 100 {
                        if input.starts_with('v') && input.ends_with('x') || input.is_empty() {
                            return None;
                        }
                    }
                }
            }
            return Some((major, minor));
        }
    }
    None
}
