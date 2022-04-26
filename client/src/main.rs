#![feature(map_first_last)]
#![feature(maybe_uninit_slice)]

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

#[derive(Debug)]
struct IcmpEchoHeader {
    msg_type: u8,
    code: u8,
    checksum: u16,
    identifier: u16,
    sequence_number: u16,
    // We won't send any data.
}
impl IcmpEchoHeader {
    fn new(identifier: u16, sequence_number: u16) -> IcmpEchoHeader {
        let mut header = IcmpEchoHeader {
            // https://www.iana.org/assignments/icmp-parameters/icmp-parameters.xhtml
            // ECHO = 8, ECHO_REPLY = 0
            msg_type: 8,
            code: 0,
            checksum: 0,
            identifier: identifier,
            sequence_number: sequence_number,
        };
        header.checksum = header.compute_checksum();
        return header;
    }

    // Marshall out of a big endian buffer
    fn from(be_recv_buf: [MaybeUninit<u8>; 8]) -> IcmpEchoHeader {
        let be_safe_buf = unsafe { MaybeUninit::slice_assume_init_ref(&be_recv_buf) };
        let mut safe_buf: [u8; 8] = [0; 8];
        for i in 0..8 {
            safe_buf[i] = be_safe_buf[i];
        }
        // Words on x86 and x86_64 appear to be 16-bit.
        // Thus, we swap every other byte as we de-serialize.
        let header = IcmpEchoHeader {
            msg_type: safe_buf[1],
            code: safe_buf[0],
            checksum: u16::from(safe_buf[3]) << 8 | u16::from(safe_buf[2]),
            identifier: u16::from(safe_buf[5]) << 8 | u16::from(safe_buf[4]),
            sequence_number: u16::from(safe_buf[7]) << 8 | u16::from(safe_buf[6]),
        };
        return header;
    }

    // Splits the header into 16-bit values and return the 1's complement.
    fn compute_checksum(&self) -> u16 {
        // Turn type and code into a single 16-bit field.
        let mut type_and_code_concatenated: u16 = u16::from(self.msg_type) << 8;
        // Put code in the lower 8 bits;
        type_and_code_concatenated += u16::from(self.code);

        // Compute the checksum.
        let mut sum: u16 = 0;
        sum += type_and_code_concatenated;
        sum += 0; // Use 0 for checksum when computing the checksum.
        sum += self.identifier;
        sum += self.sequence_number;

        // Return the 1's complement of the sum.
        return !sum;
    }

    fn serialize(&self) -> [u8; 8] {
        // `IcmpEchoHeader` is 8B
        let mut buf: [u8; 8] = [0; 8];
        let u8mask: u16 = 0xff;
        buf[0] = self.msg_type;
        buf[1] = self.code;
        buf[2] = (self.checksum >> 8 & u8mask) as u8;
        buf[3] = (self.checksum & u8mask) as u8;
        buf[4] = (self.identifier >> 8 & u8mask) as u8;
        buf[5] = (self.identifier & u8mask) as u8;
        buf[6] = (self.sequence_number >> 8 & u8mask) as u8;
        buf[7] = (self.sequence_number & u8mask) as u8;
        return buf;
    }
}

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

    // ICMP is connectionless, so share a single socket to keep things simple.
    // (If we use multiple sockets, responses can land on any of them)
    let socket = Arc::new(Mutex::new(
        Socket::new(Domain::IPV4, Type::RAW, Some(Protocol::ICMPV4)).unwrap(),
    ));
    let ping_timeout = Duration::from_millis(config::PING_TIMEOUT_MSEC);
    socket
        .lock()
        .unwrap()
        .set_write_timeout(Some(ping_timeout))
        .unwrap();
    socket
        .lock()
        .unwrap()
        .set_read_timeout(Some(ping_timeout))
        .unwrap();

    let mut thread_id = 0;
    for hostname in config::PING_DESTINATION {
        ping_data.lock().unwrap().add_hostname(&hostname);
        let hostname_threadlocal = hostname.to_string();
        let socket_threadlocal = socket.clone();
        let ping_data_threadlocal = ping_data.clone();
        thread::spawn(move || {
            repeatedly_ping(
                hostname_threadlocal,
                thread_id,
                socket_threadlocal,
                ping_timeout,
                ping_data_threadlocal,
            )
        });
        thread_id += 1;
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
fn repeatedly_ping(
    hostname: String,
    identifier: u16,
    socket: Arc<Mutex<Socket>>,
    ping_timeout: Duration,
    ping_data: Arc<Mutex<PingData>>,
) {
    let dest_ip = *lookup_host(&hostname).unwrap().first().unwrap();
    let dest_addr_v1 = SocketAddr::new(dest_ip, 0);
    let dest_addr_v2: SockAddr = dest_addr_v1.into();
    let mut recv_buf = [MaybeUninit::new(0); 8];
    let mut sequence_number: u16 = 0;
    loop {
        let start_time = Utc::now();
        let deadline = start_time + chrono_Duration::from_std(ping_timeout).unwrap();
        // Construct an ICMP Ping header.
        let hdr = IcmpEchoHeader::new(identifier, sequence_number);
        sequence_number += 1;
        // Send the ping.
        let send_res = socket
            .lock()
            .unwrap()
            .send_to(&hdr.serialize(), &dest_addr_v2);
        match send_res {
            Ok(_size) => {}
            Err(err) => eprintln!("Error while sending to {} - {:?}", dest_addr_v1, err),
        }
        // Wait until our remote's response is available.
        while Utc::now() < deadline {
            let peek_res = socket.lock().unwrap().peek_from(&mut recv_buf);
            let peek_remote_address = peek_res.unwrap().1.as_socket().unwrap();
            if peek_remote_address == dest_addr_v1 {
                // Our remote's response is available.
                break;
            }
        }
        if Utc::now() < deadline {
            // Recv the response.
            let recv_res = socket.lock().unwrap().recv_from(&mut recv_buf);
            match recv_res {
                Ok((size, origin_addr)) => {
                    let recv_origin = origin_addr.as_socket().unwrap();
                    if size != 8 || recv_origin != dest_addr_v1 {
                        eprintln!(
                            "Recv expected 8B from {} but got {}B from {}",
                            dest_addr_v1, size, recv_origin,
                        );
                    }
                    let response = IcmpEchoHeader::from(recv_buf);
                    if response.msg_type != 0 || response.code != 69 {
                        eprintln!("Unexpected values in ICMP Echo Reply {:?}", response);
                    }
                }
                Err(err) => eprintln!("Error while recving - {:?}", err),
            }
        }
        // Determine the time it's been.
        let ping_duration = (Utc::now() - start_time).to_std().unwrap();
        // Store the ping duration.
        ping_data
            .lock()
            .unwrap()
            .add_entry(&hostname, start_time, ping_duration);
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
