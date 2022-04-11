pub const PING_DESTINATION: [&str; 2] = [
    "http://192.168.1.1",
    "http://ping.projects.chrisjeakle.com/ping/",
];
pub const ECHO_INTERFACES: [&str; 2] = ["eth0", "wlan0"];
pub const SEC_BETWEEN_PINGS: u64 = 5;
pub const PING_TIMEOUT_MSEC: u64 = 1_000;
pub const MAX_ENTRIES_SAVED: usize = 30 * 24 * (60 / SEC_BETWEEN_PINGS as usize); // 30 days
pub const WEB_UI_PORT: u16 = 8180;
