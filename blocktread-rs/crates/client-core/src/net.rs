//! QUIC session on a background tokio thread. The game polls `NetClient::rx` once per frame.
//!
//! `latency` adds artificial one-way delay in both directions, for testing prediction and
//! interpolation under realistic ping on a local machine.

use anyhow::{Context, Result, anyhow};
use bytes::Bytes;
use crossbeam_channel::{Receiver, Sender, unbounded};
use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};
use blocktread_net::{read_frame, write_frame};
use blocktread_shared::protocol::{ClientMsg, InputPacket, ServerMsg, Snapshot, decode, encode};

pub enum NetIn {
    Msg(ServerMsg),
    Snap(Snapshot, usize),
    Closed(String),
}

enum NetOut {
    Input(InputPacket),
    Msg(ClientMsg),
    Leave,
}

pub struct NetClient {
    pub rx: Receiver<NetIn>,
    tx: tokio::sync::mpsc::UnboundedSender<NetOut>,
    rtt_us: Arc<AtomicU32>,
    latency: Duration,
}

impl NetClient {
    /// Start connecting in the background. Failures arrive as `NetIn::Closed`.
    pub fn connect(addr: String, hello: ClientMsg, latency: Duration) -> NetClient {
        let (in_tx, in_rx) = unbounded();
        let (out_tx, out_rx) = tokio::sync::mpsc::unbounded_channel();
        let rtt_us = Arc::new(AtomicU32::new(0));
        let rtt = rtt_us.clone();
        let fail_tx = in_tx.clone();
        std::thread::Builder::new()
            .name("blocktread-client-net".into())
            .spawn(move || {
                let rt = match tokio::runtime::Builder::new_multi_thread().worker_threads(1).enable_all().build() {
                    Ok(rt) => rt,
                    Err(e) => {
                        let _ = fail_tx.send(NetIn::Closed(e.to_string()));
                        return;
                    }
                };
                let res = rt.block_on(run(addr, hello, in_tx, out_rx, rtt, latency));
                let reason = match res {
                    Ok(()) => "连接已关闭".to_string(),
                    Err(e) => friendly(&e),
                };
                let _ = fail_tx.send(NetIn::Closed(reason));
            })
            .expect("spawn network thread");
        NetClient { rx: in_rx, tx: out_tx, rtt_us, latency }
    }

    pub fn send_input(&self, pkt: InputPacket) {
        let _ = self.tx.send(NetOut::Input(pkt));
    }

    /// A message on the reliable stream (queue, pick, ...).
    pub fn send(&self, msg: ClientMsg) {
        let _ = self.tx.send(NetOut::Msg(msg));
    }

    pub fn leave(&self) {
        let _ = self.tx.send(NetOut::Leave);
    }

    /// Round-trip time in seconds, including any simulated latency.
    pub fn rtt(&self) -> f32 {
        self.rtt_us.load(Ordering::Relaxed) as f32 / 1e6 + self.latency.as_secs_f32() * 2.0
    }
}

fn friendly(e: &anyhow::Error) -> String {
    let s = format!("{e:#}");
    if s.contains("timed out") || s.contains("deadline") {
        "连接超时。请确认地址和端口无误、服务器正在运行，并且防火墙放行了 UDP。".into()
    } else if s.contains("refused") {
        "连接被拒绝，这个端口上没有服务器在运行。".into()
    } else {
        format!("错误信息：{s}")
    }
}

/// Holds items until their release time; preserves order.
struct Delay<T> {
    q: VecDeque<(Instant, T)>,
}

async fn run(
    addr: String,
    hello: ClientMsg,
    in_tx: Sender<NetIn>,
    mut out_rx: tokio::sync::mpsc::UnboundedReceiver<NetOut>,
    rtt: Arc<AtomicU32>,
    latency: Duration,
) -> Result<()> {
    let target = tokio::net::lookup_host(addr.as_str()).await.with_context(|| format!("无法解析地址 {addr}"))?.next().ok_or_else(|| anyhow!("无法解析地址 {addr}"))?;
    let ep = blocktread_net::client_endpoint(target)?;
    let conn = tokio::time::timeout(Duration::from_secs(5), ep.connect(target, "blocktread")?).await.context("timed out")??;
    let (mut send, mut recv) = conn.open_bi().await?;
    write_frame(&mut send, &encode(&hello)).await?;

    // incoming items pass through one ordered delay line so simulated latency never reorders them
    let (dl_tx, mut dl_rx) = tokio::sync::mpsc::unbounded_channel::<NetIn>();
    let fwd_tx = in_tx.clone();
    tokio::spawn(async move {
        let mut line = Delay { q: VecDeque::new() };
        loop {
            let next_due = line.q.front().map(|(t, _)| *t);
            tokio::select! {
                m = dl_rx.recv() => match m {
                    Some(m) => line.q.push_back((Instant::now() + latency, m)),
                    None => break,
                },
                _ = async { match next_due { Some(t) => tokio::time::sleep_until(t.into()).await, None => std::future::pending().await } } => {
                    while let Some((t, _)) = line.q.front() {
                        if *t > Instant::now() { break; }
                        let (_, m) = line.q.pop_front().unwrap();
                        if fwd_tx.send(m).is_err() { return; }
                    }
                }
            }
        }
    });

    let rel_tx = dl_tx.clone();
    let reliable = async move {
        while let Some(f) = read_frame(&mut recv).await? {
            if let Some(m) = decode::<ServerMsg>(&f) {
                let _ = rel_tx.send(NetIn::Msg(m));
            }
        }
        Ok::<_, anyhow::Error>(())
    };
    let dconn = conn.clone();
    let dg_tx = dl_tx.clone();
    let datagrams = async move {
        loop {
            let d: Bytes = dconn.read_datagram().await?;
            if let Some(s) = decode::<Snapshot>(&d) {
                let _ = dg_tx.send(NetIn::Snap(s, d.len()));
            }
        }
        #[allow(unreachable_code)]
        Ok::<_, anyhow::Error>(())
    };
    let wconn = conn.clone();
    let writer = async move {
        while let Some(o) = out_rx.recv().await {
            match o {
                NetOut::Input(pkt) => {
                    let b = Bytes::from(encode(&pkt));
                    if latency.is_zero() {
                        let _ = wconn.send_datagram(b);
                    } else {
                        let c = wconn.clone();
                        tokio::spawn(async move {
                            tokio::time::sleep(latency).await;
                            let _ = c.send_datagram(b);
                        });
                    }
                }
                NetOut::Msg(m) => {
                    // lobby messages are rare: holding the writer for the simulated delay is fine
                    if !latency.is_zero() {
                        tokio::time::sleep(latency).await;
                    }
                    write_frame(&mut send, &encode(&m)).await?;
                }
                NetOut::Leave => {
                    let _ = write_frame(&mut send, &encode(&ClientMsg::Leave)).await;
                    let _ = send.finish();
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    break;
                }
            }
        }
        Ok::<_, anyhow::Error>(())
    };
    let pconn = conn.clone();
    let ping = async move {
        loop {
            rtt.store(pconn.rtt().as_micros().min(u32::MAX as u128) as u32, Ordering::Relaxed);
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        #[allow(unreachable_code)]
        Ok::<_, anyhow::Error>(())
    };
    let res = tokio::select! {
        r = reliable => r,
        r = datagrams => r,
        r = writer => r,
        r = ping => r,
    };
    conn.close(0u32.into(), b"bye");
    ep.wait_idle().await;
    res.map_err(|e| {
        if let Some(ce) = e.downcast_ref::<quinn::ConnectionError>() {
            match ce {
                quinn::ConnectionError::ApplicationClosed(c) => anyhow!("服务器关闭了连接（{}）", String::from_utf8_lossy(&c.reason)),
                quinn::ConnectionError::TimedOut => anyhow!("timed out"),
                other => anyhow!("{other}"),
            }
        } else {
            e
        }
    })
}
