//! Signing up over a real connection, then coming back with the token.

use std::time::{Duration, Instant};
use blocktread_client_core::{ClientCore, ConnState};
use blocktread_server::store::Accounts;
use blocktread_server::{HubConfig, ServerConfig, start};
use blocktread_shared::flight::InputCmd;
use blocktread_shared::protocol::AuthReq;

fn pump(core: &mut ClientCore, mut done: impl FnMut(&ClientCore) -> bool) {
    let t0 = Instant::now();
    while !done(core) {
        core.update(0.016, &mut |_, t| InputCmd { tick: t, ..Default::default() });
        assert!(t0.elapsed() < Duration::from_secs(10), "timed out: {:?}", core.state);
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn sign_up_then_come_back_with_the_token() {
    let hub = HubConfig { accounts: Accounts::Memory, ..HubConfig::default() };
    let server = start(ServerConfig { bind: "127.0.0.1:0".parse().unwrap(), hub, verbose: false, status: None, data: None }).unwrap();
    let addr = server.addr.to_string();

    let mut a = ClientCore::connect(&addr, "随便", Duration::ZERO);
    pump(&mut a, |c| matches!(c.state, ConnState::Lobby));
    a.auth(AuthReq::Register { user: "老王".into(), pass: "secret1".into() });
    pump(&mut a, |c| c.account.is_some() || c.auth_error.is_some());
    assert_eq!(a.account.as_ref().map(|x| x.0.as_str()), Some("老王"), "{:?}", a.auth_error);
    let token = a.new_token.take().expect("a token to keep");

    let mut b = ClientCore::connect(&addr, "别的名字", Duration::ZERO);
    pump(&mut b, |c| matches!(c.state, ConnState::Lobby));
    b.auth(AuthReq::Resume { token });
    pump(&mut b, |c| c.account.is_some() || c.auth_error.is_some());
    assert_eq!(b.account.map(|x| x.0), Some("老王".to_string()));
}
