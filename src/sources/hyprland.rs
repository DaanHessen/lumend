use super::Backoff;
use crate::events::Event;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::time::Duration;

fn socket_dir() -> Option<PathBuf> {
    let signature = std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE")?;
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")?;
    Some(PathBuf::from(runtime).join("hypr").join(signature))
}

pub fn available() -> bool {
    socket_dir().is_some_and(|d| d.join(".socket2.sock").exists())
}

pub fn parse_line(line: &str) -> Vec<Event> {
    let Some((name, data)) = line.split_once(">>") else {
        return Vec::new();
    };
    match name {
        "activewindow" => {
            let class = data.split_once(',').map_or(data, |(c, _)| c).trim();
            let app = (!class.is_empty()).then(|| class.to_owned());
            vec![Event::ActiveApp(app), Event::BreakMoment]
        }
        "fullscreen" => vec![Event::Fullscreen(data.trim() == "1")],
        "workspace" | "focusedmon" | "activespecial" => vec![Event::BreakMoment],
        _ => Vec::new(),
    }
}

#[derive(serde::Deserialize)]
struct ActiveWindow {
    #[serde(default)]
    class: String,
    #[serde(default)]
    fullscreen: serde_json::Value,
}

pub fn parse_active_window(json: &str) -> Vec<Event> {
    let Ok(window) = serde_json::from_str::<ActiveWindow>(json) else {
        return vec![Event::ActiveApp(None), Event::Fullscreen(false)];
    };
    let fullscreen = match window.fullscreen {
        serde_json::Value::Bool(b) => b,
        serde_json::Value::Number(n) => n.as_u64().unwrap_or(0) > 0,
        _ => false,
    };
    let app = (!window.class.is_empty()).then_some(window.class);
    vec![Event::ActiveApp(app), Event::Fullscreen(fullscreen)]
}

fn query_active_window(dir: &std::path::Path) -> std::io::Result<String> {
    let mut stream = UnixStream::connect(dir.join(".socket.sock"))?;
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    stream.write_all(b"j/activewindow")?;
    let mut reply = String::new();
    stream.read_to_string(&mut reply)?;
    Ok(reply)
}

pub fn run(tx: Sender<Event>) {
    let Some(dir) = socket_dir() else {
        tracing::info!("not running under Hyprland, window signals disabled");
        return;
    };
    let mut backoff = Backoff::new(Duration::from_secs(1), Duration::from_secs(60));
    loop {
        if let Ok(reply) = query_active_window(&dir) {
            for event in parse_active_window(&reply) {
                if tx.send(event).is_err() {
                    return;
                }
            }
        }
        match UnixStream::connect(dir.join(".socket2.sock")) {
            Ok(stream) => {
                backoff.reset();
                for line in BufReader::new(stream).lines() {
                    let Ok(line) = line else { break };
                    for event in parse_line(&line) {
                        if tx.send(event).is_err() {
                            return;
                        }
                    }
                }
                tracing::warn!("Hyprland event socket closed, reconnecting");
            }
            Err(e) => tracing::warn!("cannot connect to Hyprland event socket: {e}"),
        }
        std::thread::sleep(backoff.next_delay());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn active_window_gives_class_and_a_break() {
        assert_eq!(
            parse_line("activewindow>>kitty,~/Projects: nvim"),
            vec![Event::ActiveApp(Some("kitty".into())), Event::BreakMoment]
        );
        assert_eq!(
            parse_line("activewindow>>,"),
            vec![Event::ActiveApp(None), Event::BreakMoment]
        );
    }

    #[test]
    fn titles_with_commas_keep_the_class() {
        assert_eq!(
            parse_line("activewindow>>firefox,a, b, c")[0],
            Event::ActiveApp(Some("firefox".into()))
        );
    }

    #[test]
    fn fullscreen_and_workspace() {
        assert_eq!(parse_line("fullscreen>>1"), vec![Event::Fullscreen(true)]);
        assert_eq!(parse_line("fullscreen>>0"), vec![Event::Fullscreen(false)]);
        assert_eq!(parse_line("workspace>>3"), vec![Event::BreakMoment]);
    }

    #[test]
    fn ignores_everything_else() {
        assert!(parse_line("windowtitle>>abc").is_empty());
        assert!(parse_line("activewindowv2>>55d1c1a0").is_empty());
        assert!(parse_line("garbage").is_empty());
    }

    #[test]
    fn initial_query_handles_old_and_new_fullscreen_fields() {
        assert_eq!(
            parse_active_window(r#"{"class":"mpv","fullscreen":2}"#),
            vec![
                Event::ActiveApp(Some("mpv".into())),
                Event::Fullscreen(true)
            ]
        );
        assert_eq!(
            parse_active_window(r#"{"class":"code","fullscreen":false}"#),
            vec![
                Event::ActiveApp(Some("code".into())),
                Event::Fullscreen(false)
            ]
        );
        assert_eq!(
            parse_active_window("{}"),
            vec![Event::ActiveApp(None), Event::Fullscreen(false)]
        );
    }
}
