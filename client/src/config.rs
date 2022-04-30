pub const SEC_BETWEEN_PINGS: u64 = 5;
pub const PING_TIMEOUT_MSEC: u64 = 1_000;
pub const MAX_ENTRIES_SAVED: usize = 7 * 24 * (60 / SEC_BETWEEN_PINGS as usize); // 1 week
pub const WEB_UI_PORT: u16 = 8180;
