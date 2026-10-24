use crate::{NetEvent, Outgoing};
use anyhow::{Result, bail};
use bytes::Bytes;
use crossbeam_channel::Sender;
use std::sync::Arc;
use std::sync::atomic::{AtomicU16, Ordering};
use std::time::Duration;
use voxwing_net::{read_frame, write_frame};
use voxwing_shared::PROTOCOL_VERSION;
use voxwing_shared::protocol::{ClientMsg, InputPacket, ServerMsg, decode, encode, hello_version};

pub async fn handle(inc: quinn::Incoming, tx: Sender<NetEvent>, next_pid: Arc<AtomicU16>) {
    let remote = inc.remote_address();
    if let Err(e) = handle_inner(inc, tx, next_pid).await {
        eprintln!("[server] connection {remote} ended: {e:#}");
    }
}

async fn handle_inner(inc: quinn::Incoming, tx: Sender<NetEvent>, next_pid: Arc<AtomicU16>) -> Result<()> {
    let conn = inc.await?;
    let (mut send, mut recv) = tokio::time::timeout(Duration::from_secs(5), conn.accept_bi()).await??;
    let Some(frame) = tokio::time::timeout(Duration::from_secs(5), read_frame(&mut recv)).await?? else { return Ok(()) };
    let (version, name, watch, bal) = match decode::<ClientMsg>(&frame) {
        Some(ClientMsg::Hello { version, name, balance }) => (version, name, false, balance),
        Some(ClientMsg::Watch { version, name, balance }) => (version, name, true, balance),
        // an older client: its hello no longer parses, but its version still says why
        _ => match hello_version(&frame) {
            Some(v) => (v, String::new(), false, 0),
            None => bail!("expected Hello"),
        },
    };
    // ships are checked when picked, in the hub
    let reject = if version != PROTOCOL_VERSION {
        Some(format!("版本不匹配（服务器 {PROTOCOL_VERSION}，客户端 {version}），先把两边更新到同一版本"))
    } else if bal != voxwing_shared::balance::hash() {
        Some("数值表不一致：服务器和客户端用的不是同一份平衡数据，先把两边更新到同一版本".into())
    } else {
        None
    };
    if let Some(reason) = reject {
        write_frame(&mut send, &encode(&ServerMsg::Reject { reason })).await?;
        let _ = send.finish();
        tokio::time::sleep(Duration::from_millis(300)).await;
        return Ok(());
    }
    let pid = next_pid.fetch_add(1, Ordering::Relaxed);
    let (out_tx, mut out_rx) = tokio::sync::mpsc::unbounded_channel::<Outgoing>();
    tx.send(NetEvent::Connect { pid, name, watch, out: out_tx })?;
    println!("[server] {} {pid} joined from {}", if watch { "spectator" } else { "player" }, conn.remote_address());

    let dconn = conn.clone();
    let writer = async move {
        while let Some(o) = out_rx.recv().await {
            match o {
                Outgoing::Reliable(b) => write_frame(&mut send, &b).await?,
                Outgoing::Datagram(b) => {
                    let _ = dconn.send_datagram(b);
                }
            }
        }
        Ok::<_, anyhow::Error>(())
    };
    let dtx = tx.clone();
    let rconn = conn.clone();
    let datagrams = async move {
        loop {
            let d: Bytes = rconn.read_datagram().await?;
            if let Some(pkt) = decode::<InputPacket>(&d) {
                dtx.send(NetEvent::Input { pid, pkt })?;
            }
        }
        #[allow(unreachable_code)]
        Ok::<_, anyhow::Error>(())
    };
    let rtx = tx.clone();
    let reliable = async move {
        while let Some(f) = read_frame(&mut recv).await? {
            match decode::<ClientMsg>(&f) {
                Some(ClientMsg::Leave) => break,
                Some(msg) => rtx.send(NetEvent::Msg { pid, msg })?,
                None => {}
            }
        }
        Ok::<_, anyhow::Error>(())
    };
    let res = tokio::select! {
        r = writer => r,
        r = datagrams => r,
        r = reliable => r,
    };
    let _ = tx.send(NetEvent::Leave { pid });
    conn.close(0u32.into(), b"bye");
    println!("[server] player {pid} left");
    res
}
