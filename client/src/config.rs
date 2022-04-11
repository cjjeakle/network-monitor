pub const PING_DESTINATION: [&str; 2] = [
    "http://ping.projects.chrisjeakle.com/ping/",
    "https://ml-hot-or-cold.projects.chrisjeakle.com",
];
pub const INTERFACES_TO_MONITOR: [&str; 1] = ["eth0", "wlan0"];
pub const SEC_BETWEEN_PINGS: u64 = 5;
pub const PING_TIMEOUT_MSEC: u64 = 1_000;
pub const MAX_ENTRIES_SAVED: usize = 30 * 24 * (60 / SEC_BETWEEN_PINGS as usize); // 30 days
pub const WEB_UI_PORT: u16 = 8180;
