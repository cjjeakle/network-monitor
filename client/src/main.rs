#![feature(map_first_last)]
use std::collections::BTreeMap;
use chrono::{DateTime, Utc};
use chrono::Duration as chrono_Duration;
use std::time::Duration;
use std::sync::{Mutex};
use std::thread;
use actix_web::{get, web, App, HttpServer, Responder};


mod config;

struct PingData {
    data: BTreeMap<DateTime<Utc>, Duration>,
}
impl PingData {
    fn AddEntry(&mut self, when: DateTime<Utc>, how_long: Duration) {
        if self.data.len() >= config::MAX_ENTRIES_SAVED {
            self.data.pop_first();  // Drop the oldest entry
        }
        self.data.insert(when, how_long);
    }
    fn ForEach(self, callback: fn(&DateTime<Utc>, &Duration)) {
        for (when, how_long) in self.data {
            callback(&when, &how_long);
        }
    }
}
static mut PINGS: Mutex<PingData> = Mutex::new(PingData{
    data: BTreeMap::new()
});

#[actix_web::main] // or #[tokio::main]
async fn main() -> std::io::Result<()> {
    thread::spawn(repeatedly_ping);
    return HttpServer::new(|| App::new().service(index))
        .bind(("127.0.0.1", config::WEB_PORT))?
        .run()
        .await
}

fn repeatedly_ping() {
    loop {
        let start_time: DateTime<Utc> = Utc::now();
        let how_long = Duration::from_millis(10);  // TODO: IMPLEMENT <=====================
        // Kick off a worker thread to lock and update the map.
        thread::spawn(|| {
            let locked_data = PINGS.lock().unwrap();
            locked_data.AddEntry(start_time, how_long);
        });
        // Wait for the ping interval to elapse.
        let next_ping_time = start_time + chrono_Duration::seconds(config::SEC_BETWEEN_PINGS as i64);
        let cur_time = Utc::now();
        if cur_time < next_ping_time {
            thread::sleep((next_ping_time - cur_time).to_std().unwrap());
        }
    }
}

#[get("/index.html")]
async fn index() -> impl Responder {
    format!("Hello World!")
}
