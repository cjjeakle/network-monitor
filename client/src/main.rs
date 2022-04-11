#![feature(map_first_last)]
use actix_web::{http::header::ContentType, web, App, HttpResponse, HttpServer};
use chrono::{DateTime, Utc};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

mod config;

struct PingData {
    data: BTreeMap<String, BTreeMap<String, BTreeMap<DateTime<Utc>, Duration>>>,
}
impl PingData {
    fn add_url(&mut self, url: &str) {
        self.data.insert(url.to_string(), BTreeMap::new());
    }
    fn add_interface_to_url(&mut self, url: &String, interface: &str) {
        self.data
            .get_mut(url)
            .unwrap()
            .insert(interface.to_string(), BTreeMap::new());
    }
    fn add_entry(
        &mut self,
        url: &String,
        interface: &String,
        when: DateTime<Utc>,
        how_long: Duration,
    ) {
        let ping_results = self.data.get_mut(url).unwrap().get_mut(interface).unwrap();
        if ping_results.len() >= config::MAX_ENTRIES_SAVED {
            ping_results.pop_first(); // Drop the oldest entry
        }
        ping_results.insert(when, how_long);
    }
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    let ping_data = Arc::new(Mutex::new(PingData {
        data: BTreeMap::new(),
    }));

    for url in config::PING_DESTINATION {
        ping_data.lock().unwrap().add_url(&url);
        for interface in config::INTERFACES_TO_MONITOR {
            let url_threadlocal = url.to_string();
            let interface_threadlocal = interface.to_string();
            let ping_data_threadlocal = ping_data.clone();
            ping_data
                .lock()
                .unwrap()
                .add_interface_to_url(&url_threadlocal, interface);
            thread::spawn(move || {
                repeatedly_ping(
                    url_threadlocal,
                    interface_threadlocal,
                    ping_data_threadlocal,
                )
            });
        }
    }
    let ping_data_read_clone = web::Data::new(Arc::clone(&ping_data));
    return HttpServer::new(move || {
        App::new()
            .app_data(ping_data_read_clone.clone())
            .route("/", web::get().to(index))
    })
    .bind(("0.0.0.0", config::WEB_UI_PORT))?
    .run()
    .await;
}

// Pings the destination URI.
fn repeatedly_ping(url: String, interface: String, ping_data: Arc<Mutex<PingData>>) {
    loop {
        // Kick off a worker thread to perform a ping and append the result to `PingData`.
        let url_threadlocal = url.clone();
        let interface_threadlocal = interface.clone();
        let ping_data_threadlocal = ping_data.clone();
        thread::spawn(move || {
            let start_time: DateTime<Utc> = Utc::now();
            let _result = ureq::get(url_threadlocal.as_str())
                .timeout(Duration::from_millis(config::PING_TIMEOUT_MSEC))
                .call();
            let how_long = Utc::now() - start_time;
            ping_data_threadlocal.lock().unwrap().add_entry(
                &url_threadlocal,
                &interface_threadlocal,
                start_time,
                how_long.to_std().unwrap(),
            );
        });
        // Wait for the ping interval to elapse and repeat.
        thread::sleep(Duration::from_secs(config::SEC_BETWEEN_PINGS));
    }
}

// The web UI.
async fn index(ping_data: web::Data<Arc<Mutex<PingData>>>) -> HttpResponse {
    let locked_data = &ping_data.lock().unwrap().data;
    let mut html = String::new();

    // Style the tables
    html += "<style>
    table {
      margin: 0 auto;
    }
    table {
      color: black;
      background: white;
      border: 1px solid grey;
    }
    table caption {
      padding:.5em;
    }
    table th,
    table td {
      padding: .5em;
      border: 1px solid lightgrey;
    }
    </style>";

    // Add interface headings, each will get a column
    html += "<table><thead><tr>";
    for interface_data in locked_data {
        html += format!("<th>{}</th>", interface_data.0).as_str();
    }
    html += "</tr></thead>";
    html += "<tbody><tr>";
    // Add the per-interface data
    for url_data in locked_data {
        // Add URL headings, each will get a column
        html += "<td><table><thead><tr>";
        for interface_data in url_data.1 {
            html += format!("<th>{}</th>", interface_data.0).as_str();
        }
        html += "</tr><thead>";
        // Add the per-url data
        html += "<tbody><tr>";
        for interface_data in url_data.1 {
            // Label the per-URL ping data fields
            html += "<td><table><thead><tr><th>timestamp</th><th>duration</th><th>relative magnitude</th></tr></thead>";
            // Rows of per-URL ping data
            html += "<tbody>";
            for (timestamp, duration) in interface_data.1.iter().rev() {
                let mut i: u16 = 0;
                let log_pct_of_timeout = (f64::from(duration.as_millis() as f64)
                    .log(config::PING_TIMEOUT_MSEC as f64)
                    * 100.0) as u16;
                let mut magnitude_bars = String::new();
                while i < log_pct_of_timeout {
                    magnitude_bars += "|";
                    i += 2;
                }
                html += format!(
                    "<tr><td>{:?}</td><td>{:.3} ms</td><td>{}</td></tr>",
                    timestamp,
                    duration.as_secs_f32() * 1000.0,
                    magnitude_bars
                )
                .as_str();
            }
            html += "</tbody></table></td>"
        }
        html += "</tbody></table></td>";
    }
    html += "</tbody>";
    html += "</table>";

    return HttpResponse::Ok()
        .content_type(ContentType::html())
        .body(html);
}
