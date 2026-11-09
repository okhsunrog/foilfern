//! The UI thread of the test binary with Slint's headless platform.
//!
//! Slint's platform belongs to the thread that installs it, and its event
//! loop proxy to the whole process, so it can be installed only once per
//! process. All tests that render the application therefore run one after
//! another on one UI thread that owns the platform; each gets a fresh
//! window ([`Headless::reset()`]).

use std::cell::RefCell;
use std::panic::AssertUnwindSafe;
use std::sync::OnceLock;
use std::sync::mpsc::{self, Sender};

use librepcb_app::screenshot::{self, Headless};

/// A test closure run on the UI thread.
type Job = Box<dyn FnOnce(&mut Headless) + Send>;

/// The name of the UI thread.
const UI_THREAD: &str = "slint-headless";

thread_local! {
    /// The message of the last panic on the UI thread (with its location),
    /// recorded by the panic hook.
    static LAST_PANIC: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// Runs `test` on the UI thread with a fresh headless window of
/// `width` × `height` pixels and returns its result. A panic of `test` fails
/// the calling test with the same message.
pub fn with_headless<R: Send + 'static>(
    width: u32,
    height: u32,
    test: impl FnOnce(&Headless) -> R + Send + 'static,
) -> R {
    let (result_tx, result_rx) = mpsc::channel();
    ui_thread()
        .send(Box::new(move |headless: &mut Headless| {
            headless.reset(width, height);
            LAST_PANIC.with_borrow_mut(|p| *p = None);
            let result = std::panic::catch_unwind(AssertUnwindSafe(|| test(headless)))
                .map_err(|_| LAST_PANIC.with_borrow_mut(Option::take).unwrap_or_default());
            // The receiver waits for the result, see below.
            let _ = result_tx.send(result);
        }))
        .expect("UI thread alive");
    match result_rx.recv().expect("UI thread alive") {
        Ok(result) => result,
        Err(message) => panic!("on the UI thread: {message}"),
    }
}

/// The sender of jobs to the UI thread, started on first use.
fn ui_thread() -> &'static Sender<Job> {
    static SENDER: OnceLock<Sender<Job>> = OnceLock::new();
    SENDER.get_or_init(|| {
        // Record panic messages of the UI thread for the calling test (the
        // output of the UI thread is not captured for it).
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            if std::thread::current().name() == Some(UI_THREAD) {
                LAST_PANIC.with_borrow_mut(|p| *p = Some(info.to_string()));
            }
            previous(info);
        }));
        let (tx, rx) = mpsc::channel::<Job>();
        std::thread::Builder::new()
            .name(UI_THREAD.into())
            .spawn(move || {
                let mut headless = screenshot::install_platform(1, 1).expect("platform");
                for job in rx {
                    job(&mut headless);
                }
            })
            .expect("UI thread");
        tx
    })
}
