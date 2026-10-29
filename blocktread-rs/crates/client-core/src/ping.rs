//! Round trips to the game servers, for choosing one. Each measurement is a QUIC handshake: the
//! connection's first RTT estimate comes from it, then the client hangs up with `PING_CLOSE`. It
//! uses the game's own port, so whatever lets players in lets pings in.

use blocktread_net::Target;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Ping {
    /// Not measured yet.
    Pending,
    /// Round trip in milliseconds.
    Ms(f32),
    /// No answer within the timeout (server down, port closed, address wrong).
    Unreachable,
}

/// Measures every server again and again on a background thread while it lives.
pub struct Pinger {
    results: Arc<Mutex<Vec<Ping>>>,
    stop: Arc<AtomicBool>,
}

impl Pinger {
    /// Start measuring `servers` now and then every `every`. One whose certificate isn't the one
    /// pinned counts as unreachable.
    pub fn start(servers: Vec<Target>, every: Duration) -> Pinger {
        let results = Arc::new(Mutex::new(vec![Ping::Pending; servers.len()]));
        let stop = Arc::new(AtomicBool::new(false));
        let (r, s) = (results.clone(), stop.clone());
        let spawned = std::thread::Builder::new().name("blocktread-ping".into()).spawn(move || {
            let Ok(rt) = tokio::runtime::Builder::new_current_thread().enable_all().build() else { return };
            rt.block_on(async move {
                while !s.load(Ordering::Relaxed) {
                    let all = futures_join(servers.iter().map(|t| measure(t.clone())).collect()).await;
                    *r.lock().unwrap() = all;
                    // sleep in small steps so dropping the pinger ends the thread promptly
                    let mut left = every;
                    while left > Duration::ZERO && !s.load(Ordering::Relaxed) {
                        let step = left.min(Duration::from_millis(200));
                        tokio::time::sleep(step).await;
                        left -= step;
                    }
                }
            });
        });
        if spawned.is_err() {
            stop.store(true, Ordering::Relaxed);
        }
        Pinger { results, stop }
    }

    /// The latest measurement of server `i`.
    pub fn get(&self, i: usize) -> Ping {
        self.results.lock().unwrap().get(i).copied().unwrap_or(Ping::Pending)
    }
}

impl Drop for Pinger {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// Run the measurements side by side, results in order.
async fn futures_join(tasks: Vec<impl std::future::Future<Output = Ping> + Send + 'static>) -> Vec<Ping> {
    let handles: Vec<_> = tasks.into_iter().map(tokio::spawn).collect();
    let mut out = Vec::with_capacity(handles.len());
    for h in handles {
        out.push(h.await.unwrap_or(Ping::Unreachable));
    }
    out
}

async fn measure(server: Target) -> Ping {
    let attempt = async {
        let target = tokio::net::lookup_host(server.addr.as_str()).await.ok()?.next()?;
        let ep = blocktread_net::client_endpoint(target, server.pin).ok()?;
        let conn = ep.connect(target, "blocktread").ok()?.await.ok()?;
        let rtt = conn.rtt();
        conn.close(blocktread_net::PING_CLOSE.into(), b"ping");
        ep.wait_idle().await;
        Some(Ping::Ms(rtt.as_secs_f32() * 1000.0))
    };
    tokio::time::timeout(Duration::from_secs(3), attempt).await.ok().flatten().unwrap_or(Ping::Unreachable)
}
