use anyhow::{Context, Result, bail};
use std::net::SocketAddr;
use std::path::PathBuf;
use voxwing_server::{BUILD, HubConfig, ServerConfig, start};
use voxwing_shared::DEFAULT_PORT;

const USAGE: &str = "用法: voxwing-server [--port 4433] [--bind 0.0.0.0] [--queue-wait 15] [--match-secs 480] [--kill-limit 10] [--status 文件]
  --bind ADDR       监听的本机地址（默认 0.0.0.0，即所有网卡；127.0.0.1 只允许本机）
  --queue-wait S    排队多少秒还凑不齐人就用 AI 补位（默认 15）
  --match-secs S    一局的时长（默认 480 秒）
  --kill-limit N    先达到这么多击毁的一方提前获胜（默认 10）
  --status FILE     每 5 秒把在线人数等写进这个 JSON 文件（部署时据此判断能不能重启）

玩家在客户端里选模式（自由混战 / 2v2）开始匹配，服务器凑局、补 AI、选完机体后开打。";

fn main() -> Result<()> {
    let mut port = DEFAULT_PORT;
    let mut host = "0.0.0.0".to_string();
    let mut hub = HubConfig::default();
    let mut status: Option<PathBuf> = None;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--port" => port = args.next().context("--port 需要一个数字")?.parse()?,
            "--bind" => host = args.next().context("--bind 需要一个地址")?,
            "--queue-wait" => hub.queue_wait = args.next().context("--queue-wait 需要秒数")?.parse()?,
            "--match-secs" => hub.match_secs = args.next().context("--match-secs 需要秒数")?.parse()?,
            "--kill-limit" => hub.kill_limit = args.next().context("--kill-limit 需要一个数字")?.parse()?,
            "--status" => status = Some(args.next().context("--status 需要一个文件路径")?.into()),
            "-h" | "--help" => {
                println!("{USAGE}");
                return Ok(());
            }
            other => bail!("不认识的参数 {other}\n{USAGE}"),
        }
    }
    let bind: SocketAddr = format!("{host}:{port}").parse().with_context(|| format!("--bind 地址无效：{host}"))?;
    let handle = start(ServerConfig { bind, hub: hub.clone(), verbose: true, status })?;
    println!(
        "体素星翼服务器 {BUILD} 已启动：{} · 匹配最多等 {} 秒，人不够就用 AI 补位 · 每局 {} 秒或 {} 击毁 · UDP 端口 {}",
        handle.addr,
        hub.queue_wait,
        hub.match_secs,
        hub.kill_limit,
        handle.addr.port()
    );
    println!("玩家在客户端的「开始游戏 → 在线服务器」里填这台机器的地址 {{IP}}:{} 即可匹配。按 Ctrl+C 停止。", handle.addr.port());
    loop {
        std::thread::park();
    }
}
