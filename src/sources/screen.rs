use crate::context::ScreenStats;
use crate::events::Event;
use rustix::fs::{MemfdFlags, ftruncate, memfd_create};
use rustix::mm::{MapFlags, ProtFlags, mmap, munmap};
use std::os::fd::{AsFd, OwnedFd};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::time::Duration;
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::{
    wl_buffer::WlBuffer,
    wl_output::{self, WlOutput},
    wl_registry,
    wl_shm::{self, WlShm},
    wl_shm_pool::WlShmPool,
};
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle, WEnum, delegate_noop};
use wayland_protocols_wlr::screencopy::v1::client::{
    zwlr_screencopy_frame_v1::{self, ZwlrScreencopyFrameV1},
    zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1,
};

const GRID_STEP: usize = 8;
const BRIGHT_THRESHOLD: f64 = 0.6;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PixelOrder {
    Bgrx,
    Rgbx,
}

impl PixelOrder {
    fn from_shm(format: wl_shm::Format) -> Option<Self> {
        match format {
            wl_shm::Format::Argb8888 | wl_shm::Format::Xrgb8888 => Some(Self::Bgrx),
            wl_shm::Format::Abgr8888 | wl_shm::Format::Xbgr8888 => Some(Self::Rgbx),
            _ => None,
        }
    }
}

fn srgb_to_linear_table() -> [f64; 256] {
    let mut table = [0.0; 256];
    for (i, v) in table.iter_mut().enumerate() {
        let c = i as f64 / 255.0;
        *v = if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        };
    }
    table
}

pub fn sample(
    pixels: &[u8],
    width: usize,
    height: usize,
    stride: usize,
    order: PixelOrder,
) -> Option<ScreenStats> {
    if width == 0
        || height == 0
        || stride < width * 4
        || pixels.len() < stride * (height - 1) + width * 4
    {
        return None;
    }
    let table = srgb_to_linear_table();
    let (r, b) = match order {
        PixelOrder::Bgrx => (2, 0),
        PixelOrder::Rgbx => (0, 2),
    };
    let (mut sum, mut bright, mut count) = (0.0, 0usize, 0usize);
    for y in (GRID_STEP / 2..height).step_by(GRID_STEP) {
        let row = &pixels[y * stride..y * stride + width * 4];
        for px in row.chunks_exact(4).skip(GRID_STEP / 2).step_by(GRID_STEP) {
            let luma = 0.2126 * table[px[r] as usize]
                + 0.7152 * table[px[1] as usize]
                + 0.0722 * table[px[b] as usize];
            sum += luma;
            bright += usize::from(luma > BRIGHT_THRESHOLD);
            count += 1;
        }
    }
    (count > 0).then(|| ScreenStats {
        mean_luma: sum / count as f64,
        bright_fraction: bright as f64 / count as f64,
    })
}

struct Mapping {
    ptr: *mut std::ffi::c_void,
    len: usize,
}

impl Mapping {
    fn new(fd: &OwnedFd, len: usize) -> rustix::io::Result<Self> {
        // SAFETY: a fresh shared mapping of a memfd we own; nothing else aliases it.
        let ptr = unsafe {
            mmap(
                std::ptr::null_mut(),
                len,
                ProtFlags::READ | ProtFlags::WRITE,
                MapFlags::SHARED,
                fd,
                0,
            )?
        };
        Ok(Self { ptr, len })
    }

    fn bytes(&self) -> &[u8] {
        // SAFETY: ptr and len come from a successful mmap that lives as long as self.
        unsafe { std::slice::from_raw_parts(self.ptr.cast(), self.len) }
    }
}

impl Drop for Mapping {
    fn drop(&mut self) {
        // SAFETY: unmapping exactly the region mapped in `new`.
        let _ = unsafe { munmap(self.ptr, self.len) };
    }
}

struct Shm {
    pool: WlShmPool,
    buffer: WlBuffer,
    mapping: Mapping,
    _fd: OwnedFd,
    key: (u32, u32, u32, wl_shm::Format),
}

impl Drop for Shm {
    fn drop(&mut self) {
        self.buffer.destroy();
        self.pool.destroy();
    }
}

#[derive(Debug, Clone, Copy)]
struct BufferInfo {
    format: wl_shm::Format,
    order: PixelOrder,
    width: u32,
    height: u32,
    stride: u32,
}

#[derive(Default)]
struct Frame {
    buffer: Option<BufferInfo>,
    buffer_done: bool,
    ready: bool,
    failed: bool,
}

#[derive(Default)]
struct State {
    output_names: Vec<Option<String>>,
    frame: Frame,
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

impl Dispatch<WlOutput, usize> for State {
    fn event(
        state: &mut Self,
        _: &WlOutput,
        event: wl_output::Event,
        index: &usize,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_output::Event::Name { name } = event
            && let Some(slot) = state.output_names.get_mut(*index)
        {
            *slot = Some(name);
        }
    }
}

impl Dispatch<ZwlrScreencopyFrameV1, ()> for State {
    fn event(
        state: &mut Self,
        _: &ZwlrScreencopyFrameV1,
        event: zwlr_screencopy_frame_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let frame = &mut state.frame;
        match event {
            zwlr_screencopy_frame_v1::Event::Buffer {
                format: WEnum::Value(format),
                width,
                height,
                stride,
            } => {
                if let (None, Some(order)) = (frame.buffer, PixelOrder::from_shm(format)) {
                    frame.buffer = Some(BufferInfo {
                        format,
                        order,
                        width,
                        height,
                        stride,
                    });
                }
            }
            zwlr_screencopy_frame_v1::Event::BufferDone => frame.buffer_done = true,
            zwlr_screencopy_frame_v1::Event::Ready { .. } => frame.ready = true,
            zwlr_screencopy_frame_v1::Event::Failed => frame.failed = true,
            _ => {}
        }
    }
}

delegate_noop!(State: ignore WlShm);
delegate_noop!(State: WlShmPool);
delegate_noop!(State: ignore WlBuffer);
delegate_noop!(State: ZwlrScreencopyManagerV1);

struct Capturer {
    queue: EventQueue<State>,
    qh: QueueHandle<State>,
    state: State,
    shm: WlShm,
    manager: ZwlrScreencopyManagerV1,
    outputs: Vec<WlOutput>,
    buffer: Option<Shm>,
}

type Error = Box<dyn std::error::Error>;

impl Capturer {
    fn connect() -> Result<Self, Error> {
        let conn = Connection::connect_to_env()?;
        let (globals, mut queue) = registry_queue_init::<State>(&conn)?;
        let qh = queue.handle();
        let shm: WlShm = globals.bind(&qh, 1..=1, ())?;
        let manager: ZwlrScreencopyManagerV1 = globals.bind(&qh, 1..=3, ())?;
        let mut state = State::default();
        let outputs: Vec<WlOutput> = globals.contents().with_list(|list| {
            list.iter()
                .filter(|g| g.interface == "wl_output")
                .enumerate()
                .map(|(i, g)| globals.registry().bind(g.name, g.version.min(4), &qh, i))
                .collect()
        });
        state.output_names = vec![None; outputs.len()];
        queue.roundtrip(&mut state)?;
        Ok(Self {
            queue,
            qh,
            state,
            shm,
            manager,
            outputs,
            buffer: None,
        })
    }

    fn internal_output(&self) -> Option<&WlOutput> {
        let names = &self.state.output_names;
        let internal = names.iter().position(|n| {
            n.as_deref().is_some_and(|n| {
                n.starts_with("eDP") || n.starts_with("LVDS") || n.starts_with("DSI")
            })
        });
        self.outputs.get(internal.unwrap_or(0))
    }

    fn ensure_buffer(&mut self, info: BufferInfo) -> Result<(), Error> {
        let key = (info.width, info.height, info.stride, info.format);
        if self.buffer.as_ref().is_some_and(|b| b.key == key) {
            return Ok(());
        }
        self.buffer = None;
        let len = info.stride as usize * info.height as usize;
        let fd = memfd_create("lumend-screen", MemfdFlags::CLOEXEC)?;
        ftruncate(&fd, len as u64)?;
        let mapping = Mapping::new(&fd, len)?;
        let pool = self
            .shm
            .create_pool(fd.as_fd(), i32::try_from(len)?, &self.qh, ());
        let buffer = pool.create_buffer(
            0,
            i32::try_from(info.width)?,
            i32::try_from(info.height)?,
            i32::try_from(info.stride)?,
            info.format,
            &self.qh,
            (),
        );
        self.buffer = Some(Shm {
            pool,
            buffer,
            mapping,
            _fd: fd,
            key,
        });
        Ok(())
    }

    fn capture(&mut self) -> Result<Option<ScreenStats>, Error> {
        let output = self.internal_output().ok_or("no outputs")?.clone();
        self.state.frame = Frame::default();
        let frame = self.manager.capture_output(0, &output, &self.qh, ());
        let wait_for_done = self.manager.version() >= 3;
        while !(self.state.frame.failed
            || (self.state.frame.buffer.is_some()
                && (self.state.frame.buffer_done || !wait_for_done)))
        {
            self.queue.blocking_dispatch(&mut self.state)?;
        }
        let Some(info) = self.state.frame.buffer.filter(|_| !self.state.frame.failed) else {
            frame.destroy();
            return Ok(None);
        };
        self.ensure_buffer(info)?;
        let shm = self.buffer.as_ref().ok_or("buffer missing")?;
        frame.copy(&shm.buffer);
        while !(self.state.frame.ready || self.state.frame.failed) {
            self.queue.blocking_dispatch(&mut self.state)?;
        }
        frame.destroy();
        if self.state.frame.failed {
            return Ok(None);
        }
        Ok(sample(
            shm.mapping.bytes(),
            info.width as usize,
            info.height as usize,
            info.stride as usize,
            info.order,
        ))
    }
}

pub fn run(interval: Duration, active: Arc<AtomicBool>, tx: Sender<Event>) {
    let mut capturer = match Capturer::connect() {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("screen capture unavailable ({e}), screen content signal disabled");
            return;
        }
    };
    let mut failures = 0u32;
    loop {
        if active.load(Ordering::Relaxed) {
            match capturer.capture() {
                Ok(stats) => {
                    failures = 0;
                    if tx.send(Event::Screen(stats)).is_err() {
                        return;
                    }
                }
                Err(e) => {
                    failures += 1;
                    tracing::warn!("screen capture failed: {e}");
                    if failures >= 5 {
                        tracing::warn!("giving up on screen capture after repeated failures");
                        let _ = tx.send(Event::Screen(None));
                        return;
                    }
                }
            }
        }
        std::thread::sleep(interval);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(width: usize, height: usize, stride: usize, pixel: [u8; 4]) -> Vec<u8> {
        let mut buf = vec![0u8; stride * height];
        for y in 0..height {
            for x in 0..width {
                buf[y * stride + x * 4..y * stride + x * 4 + 4].copy_from_slice(&pixel);
            }
        }
        buf
    }

    #[test]
    fn white_and_black() {
        let white = image(64, 32, 256, [255, 255, 255, 255]);
        let s = sample(&white, 64, 32, 256, PixelOrder::Bgrx).unwrap();
        assert!((s.mean_luma - 1.0).abs() < 1e-9);
        assert_eq!(s.bright_fraction, 1.0);
        let black = image(64, 32, 256, [0, 0, 0, 255]);
        let s = sample(&black, 64, 32, 256, PixelOrder::Bgrx).unwrap();
        assert_eq!((s.mean_luma, s.bright_fraction), (0.0, 0.0));
    }

    #[test]
    fn channel_order_matters_for_luma() {
        let blue_in_bgrx = image(32, 32, 128, [255, 0, 0, 255]);
        let as_bgrx = sample(&blue_in_bgrx, 32, 32, 128, PixelOrder::Bgrx).unwrap();
        let as_rgbx = sample(&blue_in_bgrx, 32, 32, 128, PixelOrder::Rgbx).unwrap();
        assert!((as_bgrx.mean_luma - 0.0722).abs() < 1e-9);
        assert!((as_rgbx.mean_luma - 0.2126).abs() < 1e-9);
    }

    #[test]
    fn mid_grey_is_linearised() {
        let grey = image(32, 32, 128, [128, 128, 128, 255]);
        let s = sample(&grey, 32, 32, 128, PixelOrder::Rgbx).unwrap();
        assert!((s.mean_luma - 0.2158).abs() < 1e-3);
    }

    #[test]
    fn half_white_half_black() {
        let mut buf = image(64, 64, 256, [0, 0, 0, 255]);
        for y in 0..32 {
            for x in 0..64 {
                buf[y * 256 + x * 4..y * 256 + x * 4 + 3].copy_from_slice(&[255, 255, 255]);
            }
        }
        let s = sample(&buf, 64, 64, 256, PixelOrder::Bgrx).unwrap();
        assert!((s.bright_fraction - 0.5).abs() < 1e-9);
    }

    #[test]
    fn rejects_short_buffers() {
        assert!(sample(&[0; 10], 64, 32, 256, PixelOrder::Bgrx).is_none());
        assert!(sample(&[], 0, 0, 0, PixelOrder::Bgrx).is_none());
    }
}
