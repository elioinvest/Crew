//! A throwaway SSH server for trying the hosts panel without a real machine. It only listens on this
//! computer, takes user "tester" with password "right password", and runs nothing: it echoes.
//! Usage: test_sshd [port]

use std::sync::Arc;
use std::time::Duration;

use russh::keys::decode_secret_key;
use russh::server::{Auth, ChannelOpenHandle, Msg, Session};
use russh::{Channel, ChannelId};

#[derive(Default)]
struct Handler {
    line: String,
    shells: Vec<ChannelId>,
}

impl russh::server::Handler for Handler {
    type Error = anyhow::Error;

    async fn auth_password(&mut self, user: &str, password: &str) -> Result<Auth, Self::Error> {
        Ok(if user == "tester" && password == "right password" { Auth::Accept } else { Auth::reject() })
    }

    async fn channel_open_session(&mut self, _: Channel<Msg>, reply: ChannelOpenHandle, _: &mut Session) -> Result<(), Self::Error> {
        reply.accept().await;
        Ok(())
    }

    async fn exec_request(&mut self, channel: ChannelId, data: &[u8], session: &mut Session) -> Result<(), Self::Error> {
        session.channel_success(channel)?;
        session.data(channel, format!("ran: {}\n", String::from_utf8_lossy(data)).into_bytes())?;
        session.exit_status_request(channel, 0)?;
        session.eof(channel)?;
        session.close(channel)?;
        Ok(())
    }

    async fn pty_request(&mut self, channel: ChannelId, _: &str, _: u32, _: u32, _: u32, _: u32, _: &[(russh::Pty, u32)], session: &mut Session) -> Result<(), Self::Error> {
        session.channel_success(channel)?;
        Ok(())
    }

    async fn shell_request(&mut self, channel: ChannelId, session: &mut Session) -> Result<(), Self::Error> {
        session.channel_success(channel)?;
        self.shells.push(channel);
        session.data(channel, b"welcome\r\ntester$ ".to_vec())?;
        Ok(())
    }

    async fn data(&mut self, channel: ChannelId, data: &[u8], session: &mut Session) -> Result<(), Self::Error> {
        if !self.shells.contains(&channel) {
            return Ok(());
        }
        for b in data {
            if *b == b'\r' {
                let reply = format!("\r\nyou ran [{}]\r\ntester$ ", self.line);
                self.line.clear();
                session.data(channel, reply.into_bytes())?;
            } else {
                self.line.push(*b as char);
                session.data(channel, vec![*b])?;
            }
        }
        Ok(())
    }
}

#[tokio::main]
async fn main() {
    let port: u16 = std::env::args().nth(1).and_then(|p| p.parse().ok()).unwrap_or(0);
    let key = decode_secret_key(include_str!("../tests/test_host_key"), None).unwrap();
    let config = Arc::new(russh::server::Config { auth_rejection_time: Duration::from_millis(10), auth_rejection_time_initial: Some(Duration::ZERO), keys: vec![key], ..Default::default() });
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await.unwrap();
    println!("{}", listener.local_addr().unwrap().port());
    loop {
        let Ok((stream, _)) = listener.accept().await else { return };
        let config = config.clone();
        tokio::spawn(async move {
            if let Ok(session) = russh::server::run_stream(config, stream, Handler::default()).await {
                let _ = session.await;
            }
        });
    }
}
