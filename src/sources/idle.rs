use crate::events::Event;
use std::sync::mpsc::Sender;
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::{wl_registry, wl_seat::WlSeat};
use wayland_client::{Connection, Dispatch, QueueHandle, delegate_noop};
use wayland_protocols::ext::idle_notify::v1::client::{
    ext_idle_notification_v1::{self, ExtIdleNotificationV1},
    ext_idle_notifier_v1::ExtIdleNotifierV1,
};

struct State {
    tx: Sender<Event>,
    alive: bool,
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
    fn event(
        _: &mut Self,
        _: &wl_registry::WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ExtIdleNotificationV1, ()> for State {
    fn event(
        state: &mut Self,
        _: &ExtIdleNotificationV1,
        event: ext_idle_notification_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let idle = match event {
            ext_idle_notification_v1::Event::Idled => true,
            ext_idle_notification_v1::Event::Resumed => false,
            _ => return,
        };
        state.alive = state.tx.send(Event::Idle(idle)).is_ok();
    }
}

delegate_noop!(State: ignore WlSeat);
delegate_noop!(State: ExtIdleNotifierV1);

pub fn run(timeout_seconds: u32, tx: Sender<Event>) {
    if let Err(e) = watch(timeout_seconds, tx) {
        tracing::warn!("idle notifications unavailable: {e}");
    }
}

fn watch(timeout_seconds: u32, tx: Sender<Event>) -> Result<(), Box<dyn std::error::Error>> {
    let conn = Connection::connect_to_env()?;
    let (globals, mut queue) = registry_queue_init::<State>(&conn)?;
    let qh = queue.handle();
    let seat: WlSeat = globals.bind(&qh, 1..=9, ())?;
    let notifier: ExtIdleNotifierV1 = globals.bind(&qh, 1..=1, ())?;
    let _notification =
        notifier.get_idle_notification(timeout_seconds.saturating_mul(1000), &seat, &qh, ());
    let mut state = State { tx, alive: true };
    while state.alive {
        queue.blocking_dispatch(&mut state)?;
    }
    Ok(())
}
