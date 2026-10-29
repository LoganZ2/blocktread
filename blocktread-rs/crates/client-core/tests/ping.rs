//! The server picker's ping: a running server answers, a port with nothing on it doesn't.

use std::time::{Duration, Instant};
use blocktread_client_core::Target;
use blocktread_client_core::ping::{Ping, Pinger};
use blocktread_server::{HubConfig, ServerConfig, start};

#[test]
fn a_server_answers_and_a_dead_port_does_not() {
    let server = start(ServerConfig { bind: "127.0.0.1:0".parse().unwrap(), hub: HubConfig::default(), verbose: false, status: None, data: None }).unwrap();
    // a port nothing listens on: bind a socket to find one, then let it go
    let dead = std::net::UdpSocket::bind("127.0.0.1:0").unwrap().local_addr().unwrap();
    let pinger = Pinger::start(vec![server.addr.to_string().into(), dead.to_string().into()], Duration::from_secs(10));
    let t0 = Instant::now();
    while (pinger.get(0) == Ping::Pending || pinger.get(1) == Ping::Pending) && t0.elapsed() < Duration::from_secs(8) {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(matches!(pinger.get(0), Ping::Ms(ms) if ms < 100.0), "{:?}", pinger.get(0));
    assert_eq!(pinger.get(1), Ping::Unreachable);
}

#[test]
fn a_pinned_server_is_known_by_its_certificate() {
    let dir = std::env::temp_dir().join(format!("blocktread-pin-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let at = || ServerConfig { bind: "127.0.0.1:0".parse().unwrap(), hub: HubConfig::default(), verbose: false, status: None, data: Some(dir.clone()) };
    let server = start(at()).unwrap();
    // the certificate is kept: a restart shows the same one
    let again = start(at()).unwrap();
    assert_eq!(server.fingerprint, again.fingerprint);
    let pin = blocktread_net::parse_pin(&server.fingerprint).unwrap();
    let mut wrong = pin;
    wrong[0] ^= 1;
    let addr = server.addr.to_string();
    let pinger = Pinger::start(vec![Target { addr: addr.clone(), pin: Some(pin) }, Target { addr, pin: Some(wrong) }], Duration::from_secs(10));
    let t0 = Instant::now();
    while (pinger.get(0) == Ping::Pending || pinger.get(1) == Ping::Pending) && t0.elapsed() < Duration::from_secs(8) {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(matches!(pinger.get(0), Ping::Ms(_)), "the right certificate: {:?}", pinger.get(0));
    assert_eq!(pinger.get(1), Ping::Unreachable, "anyone else's is turned down");
    let _ = std::fs::remove_dir_all(&dir);
}
