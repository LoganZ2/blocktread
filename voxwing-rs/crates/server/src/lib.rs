//! Authoritative game server. Runs as its own binary or embedded in the client (local play).
//!
//! Two threads: a tokio thread owns the QUIC endpoint and one task per connection; a plain
//! thread runs the fixed 60 Hz simulation: the hub with its queues and every match in progress.
//! They talk through channels, so the simulation never waits on the network.

mod conn;
pub mod game;
pub mod hub;

use anyhow::Result;
use bytes::Bytes;
use crossbeam_channel::Receiver;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU16, Ordering};
use std::time::{Duration, Instant};
use voxwing_shared::TICK_RATE;
use voxwing_shared::protocol::{ClientMsg, InputPacket};

pub use game::GameConfig;
pub use hub::HubConfig;

pub enum NetEvent {
    /// A connection passed the handshake. `watch`: it came to spectate.
    Connect { pid: u16, name: String, watch: bool, out: tokio::sync::mpsc::UnboundedSender<Outgoing> },
    Msg { pid: u16, msg: ClientMsg },
    Input { pid: u16, pkt: InputPacket },
    Leave { pid: u16 },
}

#[derive(Clone)]
pub enum Outgoing {
    Reliable(Bytes),
    Datagram(Bytes),
}

#[derive(Clone, Debug)]
pub struct ServerConfig {
    pub bind: SocketAddr,
    pub hub: HubConfig,
    pub verbose: bool,
    /// A JSON file kept up to date with who is online (for deploys: restart only when empty).
    pub status: Option<PathBuf>,
}

/// This build: the release tag CI builds it for (`VOXWING_BUILD`), else the crate version.
pub const BUILD: &str = match option_env!("VOXWING_BUILD") {
    Some(b) => b,
    None => env!("CARGO_PKG_VERSION"),
};

pub struct ServerHandle {
    pub addr: SocketAddr,
    stop: Arc<AtomicBool>,
}

impl ServerHandle {
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

impl Drop for ServerHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

pub fn start(cfg: ServerConfig) -> Result<ServerHandle> {
    let (net_tx, net_rx) = crossbeam_channel::unbounded::<NetEvent>();
    let stop = Arc::new(AtomicBool::new(false));
    let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<SocketAddr>>();
    let stop_net = stop.clone();
    let bind = cfg.bind;
    std::thread::Builder::new().name("voxwing-net".into()).spawn(move || {
        let rt = match tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build() {
            Ok(rt) => rt,
            Err(e) => {
                let _ = ready_tx.send(Err(e.into()));
                return;
            }
        };
        rt.block_on(async move {
            let ep = match voxwing_net::server_endpoint(bind) {
                Ok(ep) => ep,
                Err(e) => {
                    let _ = ready_tx.send(Err(e));
                    return;
                }
            };
            let _ = ready_tx.send(ep.local_addr().map_err(Into::into));
            let next_pid = Arc::new(AtomicU16::new(1));
            let mut poll = tokio::time::interval(Duration::from_millis(200));
            loop {
                tokio::select! {
                    inc = ep.accept() => match inc {
                        Some(inc) => { tokio::spawn(conn::handle(inc, net_tx.clone(), next_pid.clone())); }
                        None => break,
                    },
                    _ = poll.tick() => if stop_net.load(Ordering::Relaxed) { break },
                }
            }
            ep.close(0u32.into(), b"server stopped");
            let _ = tokio::time::timeout(Duration::from_secs(1), ep.wait_idle()).await;
        });
    })?;
    let addr = ready_rx.recv()??;
    let stop_sim = stop.clone();
    let hub_cfg = cfg.hub.clone();
    let (verbose, status) = (cfg.verbose, cfg.status.clone());
    std::thread::Builder::new().name("voxwing-sim".into()).spawn(move || run(hub_cfg, net_rx, stop_sim, verbose, status))?;
    Ok(ServerHandle { addr, stop })
}

/// Write the status file: build, protocol, who is online. Written whole and renamed into place,
/// so a reader never sees half of it.
fn write_status(path: &std::path::Path, hub: &hub::Hub) {
    let (conns, matches, queued) = hub.census();
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let json = format!(
        "{{\"build\":\"{BUILD}\",\"protocol\":{},\"connections\":{conns},\"matches\":{matches},\"queued\":{queued},\"updated\":{now}}}\n",
        voxwing_shared::PROTOCOL_VERSION
    );
    let tmp = path.with_extension("tmp");
    if std::fs::write(&tmp, json).and_then(|_| std::fs::rename(&tmp, path)).is_err() {
        eprintln!("[server] could not write status to {}", path.display());
    }
}

/// The 60 Hz loop: network events in, one hub tick (every match steps), sleep to the next tick.
fn run(cfg: HubConfig, rx: Receiver<NetEvent>, stop: Arc<AtomicBool>, verbose: bool, status: Option<PathBuf>) {
    let mut hub = hub::Hub::new(cfg);
    let step = Duration::from_secs_f64(1.0 / TICK_RATE as f64);
    let mut next = Instant::now();
    let (mut worst, mut total, mut n) = (Duration::ZERO, Duration::ZERO, 0u32);
    let mut report = Instant::now();
    // the status file: written now (a deploy checks it right after a restart), then every 5 s
    let mut status_at = Instant::now() - Duration::from_secs(5);
    while !stop.load(Ordering::Relaxed) {
        if let Some(p) = &status
            && status_at.elapsed() >= Duration::from_secs(5)
        {
            write_status(p, &hub);
            status_at = Instant::now();
        }
        while let Ok(ev) = rx.try_recv() {
            hub.handle(ev);
        }
        let t0 = Instant::now();
        hub.tick();
        let el = t0.elapsed();
        worst = worst.max(el);
        total += el;
        n += 1;
        if verbose && report.elapsed() > Duration::from_secs(10) {
            let (conns, matches, queued) = hub.census();
            println!(
                "[server] connections {conns} · matches {matches} · queued {queued} · tick avg {:.2} ms, worst {:.2} ms",
                total.as_secs_f64() * 1000.0 / n as f64,
                worst.as_secs_f64() * 1000.0
            );
            report = Instant::now();
            (worst, total, n) = (Duration::ZERO, Duration::ZERO, 0);
        }
        next += step;
        let now = Instant::now();
        if next > now {
            std::thread::sleep(next - now);
        } else if now - next > Duration::from_millis(250) {
            // fell far behind (a stall): don't race to catch up
            next = now;
        }
    }
}
