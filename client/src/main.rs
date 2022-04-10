#![feature(map_first_last)]
use actix_web::{web, App, HttpServer, Responder};
use chrono::Duration as chrono_Duration;
use chrono::{DateTime, Utc};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

mod config;

struct PingData {
    data: BTreeMap<DateTime<Utc>, Duration>,
}
impl PingData {
    fn add_entry(&mut self, when: DateTime<Utc>, how_long: Duration) {
        if self.data.len() >= config::MAX_ENTRIES_SAVED {
            self.data.pop_first(); // Drop the oldest entry
        }
        self.data.insert(when, how_long);
    }
    fn for_each(self, callback: fn(&DateTime<Utc>, &Duration)) {
        for (when, how_long) in self.data {
            callback(&when, &how_long);
        }
    }
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    let ping_data = Arc::new(Mutex::new(PingData {
        data: BTreeMap::new(),
    }));
    let ping_data_write_clone = Arc::clone(&ping_data);
    thread::spawn(move || repeatedly_ping(ping_data_write_clone));
    let ping_data_read_clone = web::Data::new(Arc::clone(&ping_data));
    return HttpServer::new(move || {
        App::new()
            .app_data(ping_data_read_clone.clone())
            .route("/", web::get().to(index))
    })
    .bind(("127.0.0.1", config::WEB_UI_PORT))?
    .run()
    .await;
}

// Pings the destination URI.
fn repeatedly_ping(ping_data: Arc<Mutex<PingData>>) {
    loop {
        // Kick off a worker thread to perform a ping and append the result to `PingData`.
        let ping_data_write_clone = Arc::clone(&ping_data);
        thread::spawn(move || {
            let start_time: DateTime<Utc> = Utc::now();
            let reponse = ureq::get(config::PING_DESTINATION).call().unwrap();
            let how_long = Utc::now() - start_time;
            ping_data_write_clone
                .lock()
                .unwrap()
                .add_entry(start_time, how_long.to_std().unwrap());
        });
        // Wait for the ping interval to elapse and repeat.
        thread::sleep(
            chrono_Duration::seconds(config::SEC_BETWEEN_PINGS as i64)
                .to_std()
                .unwrap(),
        );
    }
}

// The web UI.
async fn index(ping_data: web::Data<Arc<Mutex<PingData>>>) -> impl Responder {
    format!(
        "Hello World! Ping records: {:?}",
        ping_data.lock().unwrap().data.len()
    )
}
