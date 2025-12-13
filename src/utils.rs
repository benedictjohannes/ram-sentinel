use byte_unit::Byte;
use std::sync::OnceLock;
use sysinfo::{System, RefreshKind, MemoryRefreshKind};

static TOTAL_MEMORY: OnceLock<u64> = OnceLock::new();

pub fn parse_size(s: &str) -> Option<u64> {
    Byte::parse_str(s, true).ok().map(|b| b.as_u64())
}

pub fn get_total_memory() -> u64 {
    *TOTAL_MEMORY.get_or_init(|| {
        let mut sys = System::new_with_specifics(
            RefreshKind::nothing().with_memory(MemoryRefreshKind::everything())
        );
        sys.refresh_memory();
        sys.total_memory()
    })
}