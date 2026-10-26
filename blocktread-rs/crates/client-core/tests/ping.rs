//! The server picker's ping: a running server answers, a port with nothing on it doesn't.

use std::time::{Duration, Instant};
use blocktread_client_core::ping::{Ping, Pinger};
use blocktread_server::{HubConfig, ServerConfig, start};

#[test]
fn a_server_answers_and_a_dead_port_does_not() {
    let server = start(ServerConfig { bind: "127.0.0.1:0".parse().unwrap(), hub: HubConfig::default(), verbose: false, status: None }).unwrap();
    // a port nothing listens on: bind a socket to find one, then let it go
    let dead = std::net::UdpSocket::bind("127.0.0.1:0").unwrap().local_addr().unwrap();
    let pinger = Pinger::start(vec![server.addr.to_string(), dead.to_string()], Duration::from_secs(10));
    let t0 = Instant::now();
    while (pinger.get(0) == Ping::Pending || pinger.get(1) == Ping::Pending) && t0.elapsed() < Duration::from_secs(8) {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(matches!(pinger.get(0), Ping::Ms(ms) if ms < 100.0), "{:?}", pinger.get(0));
    assert_eq!(pinger.get(1), Ping::Unreachable);
}
