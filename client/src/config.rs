pub const PING_DESTINATION: &str = "ping.projects.chrisjeakle.com/ping";
pub const INTERFACES_TO_MONITOR: [&str; 2] = ["eth0", "wifi?"];
pub const SEC_BETWEEN_PINGS: u64 = 5;
pub const MAX_ENTRIES_SAVED: usize = 30 * 24 * (60 / SEC_BETWEEN_PINGS as usize); // 30 days
pub const WEB_UI_PORT: u16 = 8180;
