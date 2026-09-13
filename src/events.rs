use crate::ambient::Sky;
use crate::context::{Power, ScreenStats};
use crate::ipc::Command;

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Backlight(u32),
    Screen(Option<ScreenStats>),
    ActiveApp(Option<String>),
    Fullscreen(bool),
    BreakMoment,
    Idle(bool),
    Power(Option<Power>),
    Network(Option<String>),
    VideoPlaying(bool),
    NightLight(Option<u32>),
    Sky(Sky),
    Command(Command),
    Tick,
}
