use anyhow::{Context, Result, bail};
use std::net::SocketAddr;
use std::path::PathBuf;
use blocktread_server::{BUILD, HubConfig, ServerConfig, start};
use blocktread_shared::DEFAULT_PORT;

const USAGE: &str = "用法: blocktread-server [--port 4433] [--bind 0.0.0.0] [--queue-wait 15] [--match-secs 480] [--win-points 100] [--status 文件]
  --bind ADDR       监听的本机地址（默认 0.0.0.0，即所有网卡；127.0.0.1 只允许本机）
  --queue-wait S    2v2 排队多少秒还凑不齐人就用 AI 补位（默认 15）
  --match-secs S    2v2 一局的时长，也是自由混战每局的时长（默认 480 秒）
  --win-points N    2v2 占点战中先拿到这么多分的队伍获胜（默认 100）
  --status FILE     每 5 秒把在线人数等写进这个 JSON 文件（部署时据此判断能不能重启）

自由混战：服务器上只有一个房间，玩家随进随出，没有 AI；场上有 2 人以上才计时，每局按击毁数排名。
2v2 占点战：排队凑局、补 AI、选完战车后开打；占住中央高台每秒得 1 分，击毁对手得 5 分。";

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
            "--win-points" => hub.win_points = args.next().context("--win-points 需要一个数字")?.parse()?,
            // 2v2 used to be won on kills: an old setup that still passes this keeps starting
            "--kill-limit" => {
                args.next();
                eprintln!("--kill-limit 已不再使用：2v2 现在按占点得分，见 --win-points");
            }
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
        "方块战车服务器 {BUILD} 已启动：{} · 自由混战每局 {} 秒 · 2v2 匹配最多等 {} 秒、每局 {} 秒或 {} 分 · UDP 端口 {}",
        handle.addr,
        hub.match_secs,
        hub.queue_wait,
        hub.match_secs,
        hub.win_points,
        handle.addr.port()
    );
    println!("玩家在客户端的「开始游戏 → 在线服务器」里填这台机器的地址 {{IP}}:{} 即可匹配。按 Ctrl+C 停止。", handle.addr.port());
    loop {
        std::thread::park();
    }
}
