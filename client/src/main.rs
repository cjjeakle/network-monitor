#![feature(map_first_last)]
use std::collections::BTreeMap;
use chrono::{DateTime, Utc};
use std::time::Duration;
use std::sync::{Mutex};
use std::thread;
use actix_web::{get, web, App, HttpServer, Responder};

mod config;

struct PingData {
    data: BTreeMap<DateTime<Utc>, Duration>,
    mutex: Mutex<()>
}
impl PingData {
    fn AddEntry(&mut self, when: DateTime<Utc>, how_long: Duration) {
        let _guard = self.mutex.lock().unwrap();
        let locked_data = &mut self.data;
        if locked_data.len() >= config::MAX_ENTRIES_SAVED {
            locked_data.pop_first();  // Drop the oldest entry
        }
        locked_data.insert(when, how_long);
    }
    fn ForEach(self, callback: fn(&DateTime<Utc>, &Duration)) {
        let _guard = self.mutex.lock().unwrap();
        let locked_data = &self.data;
        for (when, how_long) in locked_data {
            callback(when, how_long);
        }
    }
}
static mut PINGS: PingData = PingData{
    data: BTreeMap::new(),
    mutex: Mutex::new(())
};

#[actix_web::main] // or #[tokio::main]
async fn main() -> std::io::Result<()> {
    thread::spawn(ping);
    return HttpServer::new(|| App::new().service(index))
        .bind(("127.0.0.1", config::WEB_PORT))?
        .run()
        .await
}

fn ping() {
    loop {        
        let start_time: DateTime<Utc> = Utc::now();
        let how_long = Duration::from_millis(10);  // TODO
        // Kick off a thread to potentially wait to update the map.
        thread::spawn(move || {
            PINGS.AddEntry(start_time, how_long);
        });
        thread::sleep(Duration::from_secs(config::SEC_BETWEEN_PINGS));
    }
}

#[get("/index.html")]
async fn index() -> impl Responder {
    format!("Hello World!")
}
