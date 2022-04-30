#![feature(cursor_remaining)]
#![feature(map_first_last)]
#![feature(maybe_uninit_slice)]

use actix_web::{http::header::ContentType, web, App, HttpResponse, HttpServer};
use byteorder::{BigEndian, ReadBytesExt};
use chrono::Duration as chrono_Duration;
use chrono::{DateTime, Datelike, Local, Timelike, Utc};
use dns_lookup::lookup_host;
use rand::Rng;
use socket2::{Domain, Protocol, Socket, Type};
use std::cmp;
use std::collections::BTreeMap;
use std::io::Cursor;
use std::mem::MaybeUninit;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::os::unix::io::AsRawFd;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

mod config;

const IP_HEADER_SIZE: usize = 20;

struct PingData {
    hostnames_in_order: Vec<String>,
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
        // Allocate an ICMP message for an ECHO, use boring default values.
        let mut message = IcmpEchoMessage {
            // https://www.iana.org/assignments/icmp-parameters/icmp-parameters.xhtml
            // ECHO = 8, ECHO_REPLY = 0
            msg_type: 8,
            code: 0,
            checksum: 0,
            identifier: identifier,
            sequence_number: sequence_number,
            data: [0; 56],
        };
        // Set some values in the data, just for fun.
        // A nice plus: this exercises the checksum's carry-out.
        for i in 0..56 {
            message.data[i] = 0xFF - i as u8;
        }
        // Set the checksum.
        message.populate_checksum();
        return message;
    }

    // Takes the sum of this message as 16-bit words, adds back in any carry out,
    // takes the 1's complement. Then sets the resulting value in the checksum field.
    // http://www.faqs.org/rfcs/rfc1071.html is very helpful to understand the checksum's computation.
    fn populate_checksum(&mut self) {
        // Accumulate using a 32-bit variable so overflow is graceful.
        let mut sum: u32 = 0;
        // Take the sum of the message 16 bits at a time.
        let mut serialized = Cursor::new(self.serialize());
        while !serialized.is_empty() {
            sum += u32::from(serialized.read_u16::<BigEndian>().unwrap());
        }
        // So long as there is overflow, add it back into the lower 16 bits.
        while (sum >> 16) > 0 {
            sum = (sum & 0xFFFF) + (sum >> 16);
        }
        // Take the 1's complement of the sum.
        sum = !sum;
        // Truncate to 16 bits.
        self.checksum = sum as u16;
    }

    // Marshall into a buffer using network byte order (big endian).
    fn serialize(&self) -> [u8; std::mem::size_of::<IcmpEchoMessage>()] {
        let mut buf_be: [u8; std::mem::size_of::<IcmpEchoMessage>()] =
            [0; std::mem::size_of::<IcmpEchoMessage>()];
        buf_be[0] = self.msg_type;
        buf_be[1] = self.code;
        buf_be[2] = self.checksum.to_be_bytes()[0];
        buf_be[3] = self.checksum.to_be_bytes()[1];
        buf_be[4] = self.identifier.to_be_bytes()[0];
        buf_be[5] = self.identifier.to_be_bytes()[1];
        buf_be[6] = self.sequence_number.to_be_bytes()[0];
        buf_be[7] = self.sequence_number.to_be_bytes()[1];
        let buf_data_start = 8;
        for data_idx in 0..self.data.len() {
            buf_be[buf_data_start + data_idx] = self.data[data_idx];
        }
        return buf_be;
    }

    // Marshall out of a network byte order (big endian) buffer.
    fn from(buf_be: &[u8]) -> IcmpEchoMessage {
        let mut buf_be_iter = Cursor::new(buf_be);
        let mut message = IcmpEchoMessage {
            msg_type: buf_be_iter.read_u8().unwrap(),
            code: buf_be_iter.read_u8().unwrap(),
            checksum: buf_be_iter.read_u16::<BigEndian>().unwrap(),
            identifier: buf_be_iter.read_u16::<BigEndian>().unwrap(),
            sequence_number: buf_be_iter.read_u16::<BigEndian>().unwrap(),
            data: [0; 56],
        };
        for data_offset in 0..message.data.len() {
            message.data[data_offset] = buf_be_iter.read_u8().unwrap();
        }
        return message;
    }
}

// Configures `socket` to only listen for ICMP Echo Reply messages.
// Also applies a filter so `socket` will only listen for 64B ICMP Echo Reply messages from
// `src_ip_v4` that are annotated with ICMP ID == `echo_id` and ICMP Code == 0.
fn filter_icmp_replies(socket: &Socket, src_ip_v4: Ipv4Addr, icmp_msg_size: usize, echo_id: u16) {
    // Filter so the socket will only recv Echo Reply ICMP messages.
    // Echo Reply is type 0.
    let icmp_types_to_listen_for_bitmask: libc::c_int = !(1 << 0/* ICMP Echo Reply */);
    unsafe {
        libc::setsockopt(
            socket.as_raw_fd(),
            libc::SOL_RAW,
            1, /* ICMP_FILTER */
            &icmp_types_to_listen_for_bitmask as *const libc::c_int as *const libc::c_void,
            4, /* Size of the bitmask, it's 32 bits */
        );
    }
    // Use libc::BPF to filter yet further. Only recv 84B ICMP Echo Reply packets
    // (20B IP header + 64B ICMP message) that are from `src_ip_v4` and annotated with `echo_id`.
    //
    // About BPF and Packet memory layout:
    // https://www.kernel.org/doc/Documentation/networking/filter.txt
    // https://en.wikipedia.org/wiki/Ethernet_frame
    // https://en.wikipedia.org/wiki/IPv4#/media/File:IPv4_Packet-en.svg
    //
    // This bytecode was generated using `tcpdump`:
    // sudo tcpdump -nni eth0 -dd icmp and src 192.168.1.1 and ip[3] == 84 and icmp[icmptype] == 0 and icmp[icmpcode] == 0 and icmp[4:2] == 0x00FF
    // I used tcpdump's `-dd` output and regex-replaced it to be valid Rust:
    // find: `\{ (.*), (.*), (.*), (.*) \},` -> replace: `libc::sock_filter { code: $1, jt: $2, jf:$3, k:$4 },`
    // Note: you'll need to manually patch in variables like `dest_ip_v4` where appropriate.
    //
    // Annotated bytecode:
    // Skip to the 2B EtherType field in the Ethernet header,
    // if it's IPv4 (0x0800) continue, otherwise jump to exit.
    // (000) ldh      [12]
    // (001) jeq      #0x800           jt 2    jf 18
    // Skip past the 14B Ethernet header (the 8B preamble doesn't count).
    // Load 1B at offset 9 in the IP header (Protocol).
    // If it's protocol 1 (ICMP) continue, otherwise exit.
    // load 1 byte of the ID
    // (002) ldb      [23]
    // (003) jeq      #0x1             jt 4    jf 18
    // Load 4B at offset 12 in the IP Header (Source Address).
    // If it's equal to an IP of our choosing (192.168.1.1 in this case) continue, otherwise exit.
    // (004) ld       [26]
    // (005) jeq      #0xc0a80101      jt 6    jf 18
    // Load 1B at offset 3 in the IP Header (the lower byte of Total Length).
    // If the IP-layer message is 84B continue, otherwise exit.
    // (006) ldb      [17]
    // (007) jeq      #0x54            jt 8    jf 18
    // Load the 2B "flags and fragment offset" field of the IP Header
    // Use a mask to hide the flags in the upper 3 bits, and only leave the lower 13 bits set.
    // If the masked fragment offset is non-zero we will exit, otherwise continue.
    // (008) ldh      [20]
    // (009) jset     #0x1fff          jt 20   jf 10
    // I never did figure this out, but basically it loads a 1B IP header length into x.
    // (010) ldxb     4*([14]&0xf)
    // Add 14 to account for the Ethernet header.
    // Load byte at offset 0 of the ICMP header, the ICMP Type.
    // If it's 0 (Echo Reply) continue, otherwise exit.
    // (011) ldb      [x + 14]
    // (012) jeq      #0x0             jt 13   jf 18
    // Load byte 2 of the ICMP header, the ICMP code. If it's 0 continue, otherwise exit.
    // (013) ldb      [x + 15]
    // (014) jeq      #0x0             jt 15   jf 18
    // Load 2B at offset 4 in thee ICMP header, the ICMP ID.
    // If it matches an ID of our choosing (0x00FF in this case) continue, otherwise exit.
    // (015) ldh      [x + 18]
    // (016) jeq      #0xff            jt 17   jf 18
    // Indicate the criteria were fulfilled
    // (017) ret      #262144
    // Indicate we didn't fulfill the criteria
    // (018) ret      #0
    let mut bpf_bytecode: [libc::sock_filter; 19] = [
        libc::sock_filter {
            code: 0x28,
            jt: 0,
            jf: 0,
            k: 0x0000000c,
        },
        libc::sock_filter {
            code: 0x15,
            jt: 0,
            jf: 16,
            k: 0x00000800,
        },
        libc::sock_filter {
            code: 0x30,
            jt: 0,
            jf: 0,
            k: 0x00000017,
        },
        libc::sock_filter {
            code: 0x15,
            jt: 0,
            jf: 14,
            k: 0x00000001,
        },
        libc::sock_filter {
            code: 0x20,
            jt: 0,
            jf: 0,
            k: 0x0000001a,
        },
        libc::sock_filter {
            code: 0x15,
            jt: 0,
            jf: 12,
            k: u32::from_be_bytes(src_ip_v4.octets()),
        },
        libc::sock_filter {
            code: 0x30,
            jt: 0,
            jf: 0,
            k: 0x00000011,
        },
        libc::sock_filter {
            code: 0x15,
            jt: 0,
            jf: 10,
            k: (IP_HEADER_SIZE + icmp_msg_size).try_into().unwrap(),
        },
        libc::sock_filter {
            code: 0x28,
            jt: 0,
            jf: 0,
            k: 0x00000014,
        },
        libc::sock_filter {
            code: 0x45,
            jt: 8,
            jf: 0,
            k: 0x00001fff,
        },
        libc::sock_filter {
            code: 0xb1,
            jt: 0,
            jf: 0,
            k: 0x0000000e,
        },
        libc::sock_filter {
            code: 0x50,
            jt: 0,
            jf: 0,
            k: 0x0000000e,
        },
        libc::sock_filter {
            code: 0x15,
            jt: 0,
            jf: 5,
            k: 0x00000000,
        },
        libc::sock_filter {
            code: 0x50,
            jt: 0,
            jf: 0,
            k: 0x0000000f,
        },
        libc::sock_filter {
            code: 0x15,
            jt: 0,
            jf: 3,
            k: 0x00000000,
        },
        libc::sock_filter {
            code: 0x48,
            jt: 0,
            jf: 0,
            k: 0x00000012,
        },
        libc::sock_filter {
            code: 0x15,
            jt: 0,
            jf: 1,
            k: echo_id.into(),
        },
        libc::sock_filter {
            code: 0x6,
            jt: 0,
            jf: 0,
            k: 0x00040000,
        },
        libc::sock_filter {
            code: 0x6,
            jt: 0,
            jf: 0,
            k: 0x00000000,
        },
    ];
    let filter_program = libc::sock_fprog {
        len: bpf_bytecode.len().try_into().unwrap(),
        filter: bpf_bytecode.as_mut_ptr() as *mut libc::sock_filter,
    };
    let bpf_bytecode_size = bpf_bytecode.len() * std::mem::size_of::<libc::sock_filter>();
    let filter_program_size = std::mem::size_of::<libc::sock_fprog>() + bpf_bytecode_size;
    unsafe {
        libc::setsockopt(
            socket.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_ATTACH_FILTER,
            &filter_program as *const libc::sock_fprog as *const libc::c_void,
            filter_program_size.try_into().unwrap(),
        );
    }
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    // Skip the program name, all other command line args are hosts to ping.
    let hostnames_to_ping: Vec<String> = std::env::args().skip(1).collect();

    let ping_data = Arc::new(Mutex::new(PingData {
        hostnames_in_order: hostnames_to_ping.clone(),
        data: BTreeMap::new(),
    }));

    if hostnames_to_ping.is_empty() {
        panic!("\nPlease provide hostnames to ping as command line args.\n");
    }

    for hostname in hostnames_to_ping {
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

// Repeatedly pings a destination hostname.
fn repeatedly_ping(hostname: String, ping_data: Arc<Mutex<PingData>>) {
    // Set up this thread's ping metadata.
    let unique_threadlocal_id: u16 = rand::thread_rng().gen::<u16>();
    let mut sequence_number: u16 = 0;
    // Determine destination.
    // Only IPv4 is supported, the BPF filter and various header parsing depends on it.
    let dest_ip_v4 = match *lookup_host(&hostname).unwrap().first().unwrap() {
        IpAddr::V4(ip_v4) => ip_v4,
        IpAddr::V6(ip_v6) => {
            eprintln!(
                "\nOnly IPv4 addresses are supported. Host {} resolves to {}, which is not an IPv4 Address.\n",
                hostname,
                ip_v6
            );
            // We can't just panic, it'll just crash the thread. Exit the whole process.
            std::process::exit(0x1);
        }
    };
    let dest_addr_v1 = SocketAddr::new(IpAddr::V4(dest_ip_v4), 0);
    let dest_addr_v2: socket2::SockAddr = dest_addr_v1.into();
    // Set up a socket.
    // This is a raw ICMPv4 socket, it will recv all ICMP traffic to this host.
    // We will apply filters to make it behave more reasonably.
    let socket = Socket::new(Domain::IPV4, Type::RAW, Some(Protocol::ICMPV4)).unwrap();
    // Apply filters so we only recv and process relevant packets.
    filter_icmp_replies(
        &socket,
        dest_ip_v4,
        std::mem::size_of::<IcmpEchoMessage>(),
        unique_threadlocal_id,
    );
    // Set the ping timeout.
    let ping_timeout = Duration::from_millis(config::PING_TIMEOUT_MSEC);
    socket.set_write_timeout(Some(ping_timeout)).unwrap();
    socket.set_read_timeout(Some(ping_timeout)).unwrap();
    // Log important details.
    println!(
        "Pinging host {} (IP: {}) using ID {}",
        hostname, dest_ip_v4, unique_threadlocal_id
    );
    // Ping repeatedly.
    loop {
        sequence_number += 1;
        let start_time = Utc::now();
        let deadline = start_time + chrono_Duration::from_std(ping_timeout).unwrap();
        // Construct an ICMP Ping message.
        let request = IcmpEchoMessage::new(unique_threadlocal_id, sequence_number);
        // Send the ping.
        let send_res = socket.send_to(&request.serialize(), &dest_addr_v2);
        match send_res {
            Ok(_size) => {}
            Err(err) => eprintln!("Error while sending to {} - {:?}", dest_ip_v4, err),
        }
        // Wait for the response.
        // We are using a raw ICMP socket. Even with filters may see ICMPv4 Echo Replies meant for other
        // threads or processes. Thus, we recv in a loop until our remote's response is the one we recv.
        let mut response_recvd: bool = false;
        while Utc::now() < deadline && !response_recvd {
            let mut recv_buf = [MaybeUninit::new(0); 1024];
            let recv_res = socket.recv_from(&mut recv_buf);
            response_recvd = match recv_res {
                Ok((size, _origin_addr)) => {
                    let response_buf = &unsafe { MaybeUninit::slice_assume_init_ref(&recv_buf) }
                        [IP_HEADER_SIZE..size];
                    let response = IcmpEchoMessage::from(&response_buf);
                    let matching_response_found: bool = response.msg_type == 0
                        && response.code == 0
                        && response.identifier == unique_threadlocal_id
                        && response.sequence_number == sequence_number;
                    matching_response_found
                }
                Err(err) => {
                    eprintln!("Error while recving from {} - {:?}", dest_ip_v4, err);
                    false
                }
            }
        }
        // Determine how long the round trip took.
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

    // Style the tables.
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

    // Create a table to display the data.
    html += "<table><thead><tr>";

    // Use a scope to drop the lock as soon as possible.
    {
        let locked_ping_data = &ping_data.lock().unwrap();

        // Add hostname headings, each will get a column.
        for hostname in &locked_ping_data.hostnames_in_order {
            html += format!("<th>{}</th>", hostname).as_str();
        }
        html += "</tr></thead>";
        html += "<tbody><tr>";
        // Add the per-host data.
        for hostname in &locked_ping_data.hostnames_in_order {
            let hostname_data = &locked_ping_data.data[hostname.as_str()];
            // Label the per-host ping data fields.
            html += "<td><table><thead><tr><th>timestamp</th><th>duration</th><th>magnitude</th></tr></thead>";
            // Rows of per-host ping data.
            html += "<tbody>";
            for (timestamp, duration) in hostname_data.iter().rev() {
                let tens_of_ms = duration.as_millis() / 10;
                // Print a bar for every 10 ms, with a max of 10 bars.
                let mut num_bars = cmp::min(tens_of_ms, 10);
                let mut magnitude_bars = "".to_string();
                while num_bars > 0 {
                    magnitude_bars += "█";
                    num_bars -= 1;
                }
                // Anything greater than 100 MS is "off the charts", annotate that.
                if tens_of_ms > 10 {
                    magnitude_bars += "▓▒░";
                }
                let local_timestamp = DateTime::<Local>::from(timestamp.clone());
                // Add a row of ping data to the table.
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
    }

    html += "</tbody>";
    html += "</table>";

    return HttpResponse::Ok()
        .content_type(ContentType::html())
        .body(html);
}
