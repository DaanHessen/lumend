use crate::events::Event;
use std::sync::mpsc::Sender;
use std::time::Duration;
use zbus::blocking::fdo::DBusProxy;
use zbus::blocking::{Connection, Proxy};

const INTERVAL: Duration = Duration::from_secs(5);
const PREFIX: &str = "org.mpris.MediaPlayer2.";
const VIDEO_PLAYERS: [&str; 12] = [
    "mpv",
    "vlc",
    "celluloid",
    "totem",
    "haruna",
    "kodi",
    "jellyfin",
    "clapper",
    "showtime",
    "firefox",
    "chromium",
    "brave",
];

pub fn is_video_player(bus_name: &str) -> bool {
    let player = bus_name
        .strip_prefix(PREFIX)
        .unwrap_or(bus_name)
        .to_lowercase();
    VIDEO_PLAYERS.iter().any(|p| player.starts_with(p))
}

fn video_playing(conn: &Connection) -> zbus::Result<bool> {
    let names = DBusProxy::new(conn)?.list_names()?;
    for name in names
        .iter()
        .map(|n| n.as_str())
        .filter(|n| n.starts_with(PREFIX))
    {
        if !is_video_player(name) {
            continue;
        }
        let player = Proxy::new(
            conn,
            name,
            "/org/mpris/MediaPlayer2",
            "org.mpris.MediaPlayer2.Player",
        )?;
        if player
            .get_property::<String>("PlaybackStatus")
            .is_ok_and(|s| s == "Playing")
        {
            return Ok(true);
        }
    }
    Ok(false)
}

pub fn run(tx: Sender<Event>) {
    let conn = match Connection::session() {
        Ok(c) => c,
        Err(e) => {
            tracing::info!("session bus unavailable ({e}), media signal disabled");
            return;
        }
    };
    let mut last = None;
    loop {
        let playing = video_playing(&conn).unwrap_or(false);
        if !super::send_if_changed(&tx, &mut last, playing, Event::VideoPlaying) {
            return;
        }
        std::thread::sleep(INTERVAL);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_video_players() {
        assert!(is_video_player("org.mpris.MediaPlayer2.mpv"));
        assert!(is_video_player(
            "org.mpris.MediaPlayer2.firefox.instance_1_42"
        ));
        assert!(is_video_player("org.mpris.MediaPlayer2.VLC"));
        assert!(!is_video_player("org.mpris.MediaPlayer2.spotify"));
        assert!(!is_video_player("org.mpris.MediaPlayer2.playerctld"));
    }
}
