#![feature(map_first_last)]
#![feature(maybe_uninit_slice)]

use actix_web::{http::header::ContentType, web, App, HttpResponse, HttpServer};
use chrono::Duration as chrono_Duration;
use chrono::{DateTime, Datelike, Local, Timelike, Utc};
use dns_lookup::lookup_host;
use rand::Rng;
use socket2::{Domain, Protocol, SockAddr, Socket, Type};
use std::cmp;
use std::collections::BTreeMap;
use std::mem::MaybeUninit;
use std::net::SocketAddr;
use std::os::unix::io::AsRawFd;
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

#[derive(Debug)]
struct IcmpEchoMessage {
    msg_type: u8,
    code: u8,
    checksum: u16,
    identifier: u16,
    sequence_number: u16,
    data: [u8; 56], // 56 bytes, to bring the message up to the standard 64B.
}
impl IcmpEchoMessage {
    fn new(identifier: u16, sequence_number: u16) -> IcmpEchoMessage {
        let mut header = IcmpEchoMessage {
            // https://www.iana.org/assignments/icmp-parameters/icmp-parameters.xhtml
            // ECHO = 8, ECHO_REPLY = 0
            msg_type: 8,
            code: 0,
            checksum: 0,
            identifier: identifier,
            sequence_number: sequence_number,
            data: [0; 56],
        };
        *header.data.first_mut().unwrap() = 'a' as u8;
        *header.data.last_mut().unwrap() = 'z' as u8;
        header.populate_checksum();
        return header;
    }

    // Takes the sum of this message as 16-bit words, adds back in any carry out,
    // and return the 1's complement.
    fn populate_checksum(&mut self) {
        // Compute the checksum (use a 32-bit value so overflow is graceful).
        let mut sum: u32 = 0;

        // Take the sum of the header 16 bits at a time.
        let serialized = self.serialize();
        let mut i = 0;
        while i < serialized.len() - 1 {
            let upper_byte = u16::from(serialized[i] & 0xff) << 8;
            let lower_byte = u16::from(serialized[i + 1] & 0xff);
            sum += u32::from(upper_byte + lower_byte);
            i += 2;
        }
        if serialized.len() % 2 == 1 {
            sum += u32::from(*serialized.last().unwrap()) << 8;
        }

        // Add any overflow back in to the lower 16 bits.
        let mut checksum: u16 = ((sum >> 16) + (sum & 0xffff)) as u16;
        checksum += (sum >> 16) as u16;
        // Take the 1's complement of the sum.
        checksum = !checksum;
        // Set the checksum in big endian format.
        self.checksum = (checksum & 0x00ff) << 8 | (checksum & 0xff00) >> 8;
    }

    // Marshall into a buffer. Note that all values larger than 8-bits are big endian encoded.
    fn serialize(&self) -> [u8; std::mem::size_of::<IcmpEchoMessage>()] {
        // `IcmpEchoMessage` is 8B
        let mut buf: [u8; std::mem::size_of::<IcmpEchoMessage>()] =
            [0; std::mem::size_of::<IcmpEchoMessage>()];
        let u8mask: u16 = 0xff;
        buf[0] = self.msg_type;
        buf[1] = self.code;
        buf[3] = (self.checksum >> 8 & u8mask) as u8;
        buf[2] = (self.checksum & u8mask) as u8;
        buf[4] = (self.identifier >> 8 & u8mask) as u8;
        buf[5] = (self.identifier & u8mask) as u8;
        buf[6] = (self.sequence_number >> 8 & u8mask) as u8;
        buf[7] = (self.sequence_number & u8mask) as u8;

        let buf_data_start = 8;
        for data_idx in 0..self.data.len() {
            buf[buf_data_start + data_idx] = self.data[data_idx];
        }

        return buf;
    }

    // Marshall out of a buffer. Note that all values larger than 8-bits are big endian encoded.
    fn from(be_recv_buf: &[MaybeUninit<u8>]) -> IcmpEchoMessage {
        let be_safe_buf = unsafe { MaybeUninit::slice_assume_init_ref(&be_recv_buf) };
        let mut safe_buf: [u8; std::mem::size_of::<IcmpEchoMessage>()] =
            [0; std::mem::size_of::<IcmpEchoMessage>()];
        for i in 0..std::mem::size_of::<IcmpEchoMessage>() {
            safe_buf[i] = be_safe_buf[i];
        }
        // Words on x86 and x86_64 appear to be 16-bit.
        // Thus, we swap every other byte as we de-serialize.
        let mut message = IcmpEchoMessage {
            msg_type: safe_buf[0],
            code: safe_buf[1],
            checksum: u16::from(safe_buf[3]) << 8 | u16::from(safe_buf[2]),
            identifier: u16::from(safe_buf[4]) << 8 | u16::from(safe_buf[5]),
            sequence_number: u16::from(safe_buf[6]) << 8 | u16::from(safe_buf[7]),
            data: [0; 56],
        };
        for data_offset in 0..message.data.len() {
            let buf_offset = data_offset + 8;
            message.data[data_offset] = safe_buf[buf_offset];
        }
        return message;
    }
}

const IP_HEADER_SIZE: usize = 20;
const TOTAL_RECV_SIZE: usize = IP_HEADER_SIZE + std::mem::size_of::<IcmpEchoMessage>();

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

// Pings a destination hostname.
fn repeatedly_ping(hostname: String, ping_data: Arc<Mutex<PingData>>) {
    // Set up the destination.
    let dest_ip = *lookup_host(&hostname).unwrap().first().unwrap();
    let dest_addr_v1 = SocketAddr::new(dest_ip, 0);
    let dest_addr_v2: SockAddr = dest_addr_v1.into();
    // Set up the socket.
    // This is a raw ICMPv4 socket, it will recv all ICMP traffic to this host.
    // We will apply filters to make it behave more reasonably.
    let socket = Socket::new(Domain::IPV4, Type::RAW, Some(Protocol::ICMPV4)).unwrap();
    // Filter so this socket will only recv Echo Reply ICMP messages.
    // Echo Reply is type 0.
    let icmp_types_to_listen_for_bitmask: libc::c_int = !(1 << 0);
    unsafe {
        libc::setsockopt(
            socket.as_raw_fd(),
            libc::SOL_RAW,
            1, /* ICMP_FILTER */
            &icmp_types_to_listen_for_bitmask as *const libc::c_int as *const libc::c_void,
            4, /* Size of the bitmask, it's 32 bits */
        );
    }
    // Use BPF to filter yet further. Only recv 84B ICMP Echo Reply packets annotated with our
    // thread's `unique_threadlocal_id`.
    // https://www.kernel.org/doc/Documentation/networking/filter.txt
    // Generate BPF bytecode using `tcpdump`:
    // `sudo tcpdump -nni eth0 -dd icmp and src 192.168.1.1 and ip[3] == 84 and icmp[icmptype] == 0 and icmp[icmpcode] == 0`
    // let filters: [libc::sock_filter; 1] =[{
    //     code: 0 /* ICMP Echo Reply */,
    //     jt: /* jump true. */,
    //     jf: /* jump false. */,
    //     k: /* Generic field. */,
    // }]
    // libc::setsockopt(
    //     socket.as_raw_fd(),
    //     libc::SOL_SOCKET,
    //     libc::SO_ATTACH_FILTER,
    //     libc::sock_fprog { len: 1, &filters}
    // );
    // Set up the ping timeout.
    let ping_timeout = Duration::from_millis(config::PING_TIMEOUT_MSEC);
    socket.set_write_timeout(Some(ping_timeout)).unwrap();
    socket.set_read_timeout(Some(ping_timeout)).unwrap();
    // Set up this thread's ping metadata.
    let unique_threadlocal_id = rand::thread_rng().gen::<u16>();
    println!("ID {} for pings to {}", unique_threadlocal_id, dest_ip);
    let mut sequence_number: u16 = 0;
    let mut recv_buf = [MaybeUninit::new(0); 1024];
    loop {
        sequence_number += 1;
        let start_time = Utc::now();
        let deadline = start_time + chrono_Duration::from_std(ping_timeout).unwrap();
        // Construct an ICMP Ping header.
        let request = IcmpEchoMessage::new(unique_threadlocal_id, sequence_number);
        // Send the ping.
        let send_res = socket.send_to(&request.serialize(), &dest_addr_v2);
        match send_res {
            Ok(_size) => {}
            Err(err) => eprintln!("Error while sending to {} - {:?}", dest_addr_v1, err),
        }
        let mut response_recvd: bool = false;
        while Utc::now() < deadline && !response_recvd {
            // This raw ICMP socket will see all incoming ICMPv4 Echo Reply messages,
            // so we need to recv in a loop until our remote's response is the one we see.
            let recv_res = socket.recv_from(&mut recv_buf);
            response_recvd = match recv_res {
                Ok((size, origin_addr)) => {
                    let recv_origin = origin_addr.as_socket().unwrap();
                    if size != TOTAL_RECV_SIZE || recv_origin != dest_addr_v1 {
                        // This response's doesn't match what we expect (size, remote address).
                        // Drop this data and recv again.
                        false
                    } else {
                        let response = IcmpEchoMessage::from(&recv_buf[IP_HEADER_SIZE..size]);
                        let matching_response_found: bool = response.msg_type == 0
                            && response.code == 0
                            && response.identifier == unique_threadlocal_id
                            && response.sequence_number == sequence_number;
                        matching_response_found
                    }
                }
                Err(err) => {
                    eprintln!("Error while recving - {:?}", err);
                    false
                }
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
                if local_timestamp.hour12().0 { "PM" } else { "AM" },
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
