use crate::events::Event;
use serde::{Deserialize, Serialize};
use std::io::{self, BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::mpsc::{self, Sender, SyncSender};
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "lowercase")]
pub enum Request {
    Status,
    Why,
    Pause { minutes: Option<u64> },
    Resume,
    Forget,
    Key { key: crate::chord::Key },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Response {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default)]
    pub data: serde_json::Value,
}

impl Response {
    pub fn ok(data: serde_json::Value) -> Self {
        Self {
            ok: true,
            error: None,
            data,
        }
    }

    pub fn error(message: impl Into<String>) -> Self {
        Self {
            ok: false,
            error: Some(message.into()),
            data: serde_json::Value::Null,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Command {
    pub request: Request,
    pub reply: SyncSender<Response>,
}

impl PartialEq for Command {
    fn eq(&self, other: &Self) -> bool {
        self.request == other.request
    }
}

pub fn bind(path: &Path) -> io::Result<UnixListener> {
    if UnixStream::connect(path).is_ok() {
        return Err(io::Error::new(
            io::ErrorKind::AddrInUse,
            format!("another lumend is already listening on {}", path.display()),
        ));
    }
    let _ = std::fs::remove_file(path);
    let listener = UnixListener::bind(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok(listener)
}

pub fn serve(listener: UnixListener, tx: Sender<Event>) {
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        if let Err(e) = handle(stream, &tx) {
            tracing::debug!("control connection failed: {e}");
        }
    }
}

fn handle(stream: UnixStream, tx: &Sender<Event>) -> io::Result<()> {
    stream.set_read_timeout(Some(TIMEOUT))?;
    let mut line = String::new();
    BufReader::new(&stream).read_line(&mut line)?;
    let response = match serde_json::from_str::<Request>(&line) {
        Ok(request) => {
            let (reply, answer) = mpsc::sync_channel(1);
            if tx.send(Event::Command(Command { request, reply })).is_err() {
                return Ok(());
            }
            answer
                .recv_timeout(TIMEOUT)
                .unwrap_or_else(|_| Response::error("daemon did not answer in time"))
        }
        Err(e) => Response::error(format!("bad request: {e}")),
    };
    let mut writer = &stream;
    writeln!(
        writer,
        "{}",
        serde_json::to_string(&response).map_err(io::Error::other)?
    )
}

pub fn request(path: &Path, request: &Request) -> io::Result<Response> {
    let mut stream = UnixStream::connect(path)?;
    stream.set_read_timeout(Some(TIMEOUT + Duration::from_secs(1)))?;
    writeln!(
        stream,
        "{}",
        serde_json::to_string(request).map_err(io::Error::other)?
    )?;
    let mut line = String::new();
    BufReader::new(&stream).read_line(&mut line)?;
    serde_json::from_str(&line).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_wire_format() {
        assert_eq!(
            serde_json::to_string(&Request::Status).unwrap(),
            r#"{"cmd":"status"}"#
        );
        assert_eq!(
            serde_json::from_str::<Request>(r#"{"cmd":"pause","minutes":30}"#).unwrap(),
            Request::Pause { minutes: Some(30) }
        );
        assert_eq!(
            serde_json::from_str::<Request>(r#"{"cmd":"pause"}"#).unwrap(),
            Request::Pause { minutes: None }
        );
        assert!(serde_json::from_str::<Request>(r#"{"cmd":"reboot"}"#).is_err());
    }

    #[test]
    fn round_trip_over_a_socket() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.sock");
        let listener = bind(&path).unwrap();
        assert!(bind(&path).is_err(), "second daemon should be refused");
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || serve(listener, tx));
        std::thread::spawn(move || {
            for event in rx {
                if let Event::Command(c) = event {
                    let _ = c.reply.send(Response::ok(
                        serde_json::json!({ "echo": format!("{:?}", c.request) }),
                    ));
                }
            }
        });
        let response = request(&path, &Request::Resume).unwrap();
        assert!(response.ok);
        assert_eq!(response.data["echo"], "Resume");
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }
}
