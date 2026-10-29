//! Ping game servers the way the lobby's server picker does:
//!   cargo run -p blocktread_client_core --example ping -- 38.92.8.110:4433 99.246.198.62:4433

use std::time::Duration;
use blocktread_client_core::ping::{Ping, Pinger};

fn main() {
    let addrs: Vec<String> = std::env::args().skip(1).collect();
    let pinger = Pinger::start(addrs.iter().map(|a| a.as_str().into()).collect(), Duration::from_secs(1));
    for round in 1..=3 {
        std::thread::sleep(Duration::from_millis(1600));
        let line: Vec<String> = addrs.iter().enumerate().map(|(i, a)| match pinger.get(i) {
            Ping::Ms(ms) => format!("{a} {ms:.0} ms"),
            other => format!("{a} {other:?}"),
        }).collect();
        println!("round {round}: {}", line.join(" · "));
    }
}
