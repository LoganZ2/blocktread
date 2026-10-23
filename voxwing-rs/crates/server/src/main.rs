use anyhow::{Context, Result, bail};
use std::net::SocketAddr;
use voxwing_server::{ServerConfig, start};
use voxwing_shared::DEFAULT_PORT;
use voxwing_shared::protocol::Mode;

const USAGE: &str = "用法: voxwing-server [--port 4433] [--bind 0.0.0.0] [--mode coop|ffa] [--bots 4]
  --bind ADDR   监听的本机地址（默认 0.0.0.0，即所有网卡；127.0.0.1 只允许本机）
  --mode coop   合作波次：所有玩家一起对抗一波波 AI（默认）
  --mode ffa    自由混战：玩家和 AI 互相攻击，阵亡 4 秒后重生
  --bots N      自由混战时保持在场的 AI 数量（默认 4）";

fn main() -> Result<()> {
    let mut port = DEFAULT_PORT;
    let mut mode = Mode::Coop;
    let mut bots = 4;
    let mut host = "0.0.0.0".to_string();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--port" => port = args.next().context("--port 需要一个数字")?.parse()?,
            "--mode" => {
                mode = match args.next().as_deref() {
                    Some("coop") => Mode::Coop,
                    Some("ffa") => Mode::Ffa,
                    _ => bail!("--mode 只能是 coop 或 ffa\n{USAGE}"),
                }
            }
            "--bots" => bots = args.next().context("--bots 需要一个数字")?.parse()?,
            "--bind" => host = args.next().context("--bind 需要一个地址")?,
            "-h" | "--help" => {
                println!("{USAGE}");
                return Ok(());
            }
            other => bail!("不认识的参数 {other}\n{USAGE}"),
        }
    }
    let bind: SocketAddr = format!("{host}:{port}").parse().with_context(|| format!("--bind 地址无效：{host}"))?;
    let handle = start(ServerConfig { bind, mode, bots, verbose: true })?;
    println!("体素星翼服务器已启动：{} · 模式 {} · UDP 端口 {}", handle.addr, mode.label(), handle.addr.port());
    println!("同一局域网的玩家在客户端里填「本机局域网 IP:{}」即可加入。按 Ctrl+C 停止。", handle.addr.port());
    loop {
        std::thread::park();
    }
}
