use crate::events::Event;
use std::collections::HashMap;
use std::sync::mpsc::Sender;
use std::time::Duration;
use zbus::blocking::{Connection, Proxy};
use zbus::zvariant::{OwnedObjectPath, OwnedValue};

const INTERVAL: Duration = Duration::from_secs(60);
const IWD: &str = "net.connman.iwd";
const NM: &str = "org.freedesktop.NetworkManager";

type ManagedObjects = HashMap<OwnedObjectPath, HashMap<String, HashMap<String, OwnedValue>>>;

fn iwd_network(conn: &Connection) -> zbus::Result<Option<String>> {
    let manager = Proxy::new(conn, IWD, "/", "org.freedesktop.DBus.ObjectManager")?;
    let objects: ManagedObjects = manager.call("GetManagedObjects", &())?;
    for (path, interfaces) in objects {
        if !interfaces.contains_key("net.connman.iwd.Station") {
            continue;
        }
        let station = Proxy::new(conn, IWD, path, "net.connman.iwd.Station")?;
        if station.get_property::<String>("State")? != "connected" {
            continue;
        }
        let network: OwnedObjectPath = station.get_property("ConnectedNetwork")?;
        return Ok(Some(network.as_str().to_owned()));
    }
    Ok(None)
}

fn networkmanager_network(conn: &Connection) -> zbus::Result<Option<String>> {
    let nm = Proxy::new(conn, NM, "/org/freedesktop/NetworkManager", NM)?;
    let primary: OwnedObjectPath = nm.get_property("PrimaryConnection")?;
    if primary.as_str() == "/" {
        return Ok(None);
    }
    let active = Proxy::new(
        conn,
        NM,
        primary,
        "org.freedesktop.NetworkManager.Connection.Active",
    )?;
    Ok(Some(active.get_property::<String>("Id")?))
}

fn current(conn: &Connection) -> Option<String> {
    iwd_network(conn)
        .or_else(|_| networkmanager_network(conn))
        .unwrap_or(None)
}

pub fn run(tx: Sender<Event>) {
    let conn = match Connection::system() {
        Ok(c) => c,
        Err(e) => {
            tracing::info!("system bus unavailable ({e}), network signal disabled");
            return;
        }
    };
    let mut last = None;
    loop {
        if !super::send_if_changed(&tx, &mut last, current(&conn), Event::Network) {
            return;
        }
        std::thread::sleep(INTERVAL);
    }
}
