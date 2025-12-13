use byte_unit::Byte;

pub fn parse_size(s: &str) -> u64 {
    Byte::parse_str(s, true).map(|b| b.as_u64()).unwrap_or(0)
}
