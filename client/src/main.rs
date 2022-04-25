#![feature(map_first_last)]

use actix_web::{http::header::ContentType, web, App, HttpResponse, HttpServer};
use chrono::Duration as chrono_Duration;
use chrono::{DateTime, Datelike, Local, Timelike, Utc};
use dns_lookup::lookup_host;
use socket2::{Domain, Protocol, SockAddr, Socket, Type};
use std::cmp;
use std::collections::BTreeMap;
use std::mem::MaybeUninit;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

mod config;

struct PingData {
    data: BTreeMap<String, BTreeMap<DateTime<Utc>, Duration>>,
}
impl PingData {
    fn add_hostname(&mut self, hostname: &str) {
        self.data.insert(hostname.to_string(), BTreeMap::new());
    }
    fn add_entry(&mut self, hostname: &String, when: DateTime<Utc>, how_long: Duration) {
        let ping_results = self.data.get_mut(hostname).unwrap();
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

    for hostname in config::PING_DESTINATION {
        ping_data.lock().unwrap().add_hostname(&hostname);
        let hostname_threadlocal = hostname.to_string();
        let ping_data_threadlocal = ping_data.clone();
        thread::spawn(move || repeatedly_ping(hostname_threadlocal, ping_data_threadlocal));
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
fn repeatedly_ping(hostname: String, ping_data: Arc<Mutex<PingData>>) {
    let dest_ip = *lookup_host(&hostname).unwrap().first().unwrap();
    let dest_socket1 = SocketAddr::new(dest_ip, 0);
    let dest_socket2: SockAddr = dest_socket1.into();
    let socket = Socket::new(Domain::IPV4, Type::RAW, Some(Protocol::ICMPV4)).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_millis(config::PING_TIMEOUT_MSEC)))
        .unwrap();
    let mut recv_buf = [MaybeUninit::new(0); 256];
    loop {
        let start_time: DateTime<Utc> = Utc::now();
        // Send a ping.
        // Ignore errors, if this fails or times out, we'll just roll with it.
        let _err = socket.send_to(&[], &dest_socket2);
        // Recv the response.
        assert_eq!(
            socket
                .recv_from(&mut recv_buf)
                .unwrap()
                .1
                .as_socket()
                .unwrap(),
            dest_socket1
        );
        // Determine the time it took.
        let how_long = Utc::now() - start_time;
        // Store the ping duration.
        ping_data
            .lock()
            .unwrap()
            .add_entry(&hostname, start_time, how_long.to_std().unwrap());
        // Wait for the ping interval to elapse and repeat.
        let next_ping_time =
            start_time + chrono_Duration::seconds(config::SEC_BETWEEN_PINGS as i64);
        let cur_time = Utc::now();
        if next_ping_time > cur_time {
            thread::sleep((next_ping_time - cur_time).to_std().unwrap());
        }
    }
}

// The web UI.
async fn index(ping_data: web::Data<Arc<Mutex<PingData>>>) -> HttpResponse {
    let mut html = String::new();

    // Style the tables
    html += "<style>
    * {
        // Reset default margin & padding
        margin:0;
        padding:0;
    }
    html,body {
        position:relative;
    }
    table {
        width:100%;
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
        vertical-align: top;
        padding: .5em;
        border: 1px solid lightgrey;
    }
    </style>";

    let locked_data = &ping_data.lock().unwrap().data;

    // Add hostname headings, each will get a column
    html += "<table><thead><tr>";
    for hostname_data in locked_data {
        html += format!("<th>{}</th>", hostname_data.0).as_str();
    }
    html += "</tr></thead>";
    html += "<tbody><tr>";
    // Add the per-hostname data
    for (_, hostname_data) in locked_data {
        // Label the per-hostname ping data fields
        html += "<td><table><thead><tr><th>timestamp</th><th>duration</th><th>magnitude</th></tr></thead>";
        // Rows of per-hostname ping data
        html += "<tbody>";
        for (timestamp, duration) in hostname_data.iter().rev() {
            let tens_of_ms = duration.as_millis() / 10;
            // Print a bar for every 10 ms, with a max of 10 bars
            let mut num_bars = cmp::min(tens_of_ms, 10);
            let mut magnitude_bars = "".to_string();
            while num_bars > 0 {
                magnitude_bars += "█";
                num_bars -= 1;
            }
            // Anything greater than 100 MS is "off the charts", annotate that
            if tens_of_ms > 10 {
                magnitude_bars += "▓▒░";
            }
            let local_timestamp = DateTime::<Local>::from(timestamp.clone());
            html += format!(
                "<tr><td>{:02}-{:02} {:02}:{:02}:{:02} {}</td><td>{:_>6.1} ms</td><td>|{:_<10}</td></tr>",
                local_timestamp.month(),
                local_timestamp.day(),
                local_timestamp.hour12().1,
                local_timestamp.minute(),
                local_timestamp.second(),
                if local_timestamp.hour12().0 { "AM" } else { "PM" },
                duration.as_secs_f64() * 1000.0,
                magnitude_bars
            )
            .as_str();
        }
        html += "</tbody></table></td>"
    }
    html += "</tbody>";
    html += "</table>";

    return HttpResponse::Ok()
        .content_type(ContentType::html())
        .body(html);
}
