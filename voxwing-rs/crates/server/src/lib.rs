//! Authoritative game server. Runs as its own binary or embedded in the client ("create room").
//!
//! Two threads: a tokio thread owns the QUIC endpoint and one task per connection; a plain
//! thread runs the fixed 60 Hz simulation. They talk through channels, so the simulation never
//! waits on the network.

mod conn;
pub mod game;

use anyhow::Result;
use bytes::Bytes;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU16, Ordering};
use std::time::Duration;
use voxwing_shared::blocks::Design;
use voxwing_shared::protocol::{InputPacket, Mode};

pub use game::GameConfig;

pub enum NetEvent {
    /// `design` is None for a spectator.
    Join { pid: u16, name: String, design: Option<Design>, out: tokio::sync::mpsc::UnboundedSender<Outgoing> },
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
    pub mode: Mode,
    pub bots: u32,
    pub verbose: bool,
}

pub struct ServerHandle {
    pub addr: SocketAddr,
    pub mode: Mode,
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
    let game_cfg = GameConfig { mode: cfg.mode, bots: cfg.bots, verbose: cfg.verbose };
    std::thread::Builder::new().name("voxwing-sim".into()).spawn(move || game::run_loop(game_cfg, net_rx, stop_sim))?;
    Ok(ServerHandle { addr, mode: cfg.mode, stop })
}
