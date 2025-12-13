use byte_unit::Byte;

pub fn parse_size(s: &str) -> Option<u64> {
    Byte::parse_str(s, true).ok().map(|b| b.as_u64())
}