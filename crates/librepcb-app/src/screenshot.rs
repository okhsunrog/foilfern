//! Headless screenshots of the main window (no upstream counterpart).
//!
//! Renders the real UI with Slint's software renderer into a PNG, without a
//! display: for tests, CI and agents that cannot run Slint's MCP server.
//! `librepcb --screenshot out.png --project X.lpp --tab board` uses it.
//!
//! [`install_platform()`] must run before the first Slint component is
//! created (Slint's platform can be set once per process, on the thread
//! that uses it); [`Headless::reset()`] starts over with a new window.
//! Timers (the deferred actions of [`App`](crate::App)) and closures posted
//! from other threads with `slint::invoke_from_event_loop()` (worker
//! results, the embedded MCP server) are processed by
//! [`Headless::settle()`] and [`Headless::run_until()`].

use std::cell::RefCell;
use std::collections::VecDeque;
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{EventLoopProxy, Platform, WindowAdapter};
use slint::{EventLoopError, PhysicalSize, PlatformError, Rgb8Pixel};

/// Errors of the screenshot mode.
#[derive(Debug, thiserror::Error)]
pub enum ScreenshotError {
    /// The platform could not be installed (a component already exists).
    #[error("Failed to set up the headless platform: {0}")]
    Platform(#[from] slint::platform::SetPlatformError),
    /// Slint error.
    #[error(transparent)]
    Slint(#[from] PlatformError),
    /// PNG encoding or file error.
    #[error("Failed to write the screenshot: {0}")]
    Write(String),
}

/// Closures posted with `slint::invoke_from_event_loop()`.
type EventQueue = Arc<Mutex<VecDeque<Box<dyn FnOnce() + Send>>>>;

/// The window of the next Slint component, shared by the platform and
/// [`Headless`] (replaced by [`Headless::reset()`]).
type WindowSlot = Rc<RefCell<Rc<MinimalSoftwareWindow>>>;

struct HeadlessPlatform {
    window: WindowSlot,
    start: Instant,
    queue: EventQueue,
}

impl Platform for HeadlessPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, PlatformError> {
        Ok(self.window.borrow().clone())
    }

    fn duration_since_start(&self) -> Duration {
        self.start.elapsed()
    }

    fn new_event_loop_proxy(&self) -> Option<Box<dyn EventLoopProxy>> {
        Some(Box::new(HeadlessProxy {
            queue: Arc::clone(&self.queue),
        }))
    }
}

/// The event loop proxy of the headless platform: queues the closures for
/// [`Headless::process_events()`].
struct HeadlessProxy {
    queue: EventQueue,
}

impl EventLoopProxy for HeadlessProxy {
    fn quit_event_loop(&self) -> Result<(), EventLoopError> {
        Ok(())
    }

    fn invoke_from_event_loop(
        &self,
        event: Box<dyn FnOnce() + Send>,
    ) -> Result<(), EventLoopError> {
        self.queue.lock().push_back(event);
        Ok(())
    }
}

/// The headless window.
pub struct Headless {
    window: WindowSlot,
    size: PhysicalSize,
    queue: EventQueue,
}

/// Installs the headless platform with a window of `width` × `height`
/// pixels (scale factor 1).
pub fn install_platform(width: u32, height: u32) -> Result<Headless, ScreenshotError> {
    let window = Rc::new(RefCell::new(MinimalSoftwareWindow::new(
        RepaintBufferType::NewBuffer,
    )));
    let queue = EventQueue::default();
    slint::platform::set_platform(Box::new(HeadlessPlatform {
        window: Rc::clone(&window),
        start: Instant::now(),
        queue: Arc::clone(&queue),
    }))?;
    let size = PhysicalSize::new(width, height);
    Ok(Headless {
        window,
        size,
        queue,
    })
}

impl Headless {
    /// Starts over for the next Slint component (e.g. another test on the
    /// same thread): it gets a new window of `width` × `height` pixels, and
    /// closures posted so far are dropped. The platform itself can be
    /// installed only once per process.
    pub fn reset(&mut self, width: u32, height: u32) {
        *self.window.borrow_mut() = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
        self.size = PhysicalSize::new(width, height);
        self.queue.lock().clear();
    }

    /// Runs the closures posted from other threads (in order, including
    /// closures posted meanwhile). Returns whether any ran.
    pub fn process_events(&self) -> bool {
        let mut any = false;
        loop {
            // Pop first: the closure may post further closures.
            let event = self.queue.lock().pop_front();
            match event {
                Some(event) => {
                    event();
                    any = true;
                }
                None => return any,
            }
        }
    }

    /// Processes posted closures and timers until `done()` returns true or
    /// `timeout` elapsed (e.g. while another thread drives the embedded MCP
    /// server); returns whether `done()` became true.
    pub fn run_until(&self, timeout: Duration, mut done: impl FnMut() -> bool) -> bool {
        let end = Instant::now() + timeout;
        loop {
            self.process_events();
            slint::platform::update_timers_and_animations();
            if done() {
                return true;
            }
            if Instant::now() >= end {
                return false;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Processes posted closures and timers (deferred actions) and renders
    /// until nothing changes anymore (at most `rounds` frames). Returns the
    /// last frame.
    pub fn settle(&self, rounds: usize) -> Vec<Rgb8Pixel> {
        let window = self.window.borrow().clone();
        window.set_size(self.size);
        let (w, h) = (self.size.width as usize, self.size.height as usize);
        let mut buffer = vec![Rgb8Pixel::new(0, 0, 0); w * h];
        let mut last = buffer.clone();
        for round in 0..rounds {
            // Let zero-duration timers expire.
            std::thread::sleep(Duration::from_millis(2));
            let events = self.process_events();
            slint::platform::update_timers_and_animations();
            let drawn = window.draw_if_needed(|renderer| {
                renderer.render(&mut buffer, w);
            });
            if drawn {
                last.clone_from(&buffer);
            } else if round > 2 && !events && !window.has_active_animations() {
                break;
            }
            window.request_redraw();
        }
        last
    }

    /// Renders the window and writes it as PNG.
    pub fn save_png(&self, path: &Path) -> Result<(), ScreenshotError> {
        let pixels = self.settle(30);
        write_png(path, self.size.width, self.size.height, &pixels)
    }

    /// The window size in pixels.
    pub fn size(&self) -> PhysicalSize {
        self.size
    }
}

/// Writes RGB pixels as PNG.
pub fn write_png(
    path: &Path,
    width: u32,
    height: u32,
    pixels: &[Rgb8Pixel],
) -> Result<(), ScreenshotError> {
    let file = std::fs::File::create(path).map_err(|e| ScreenshotError::Write(e.to_string()))?;
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), width, height);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    let data: Vec<u8> = pixels.iter().flat_map(|p| [p.r, p.g, p.b]).collect();
    encoder
        .write_header()
        .and_then(|mut w| w.write_image_data(&data))
        .map_err(|e| ScreenshotError::Write(e.to_string()))
}
