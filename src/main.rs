//! dunst-in-gtk: a dunst-compatible notification daemon that renders
//! notifications with GTK4 windows (per-window HiDPI handling for free).
//!
//! Architecture:
//! - Main thread: GTK main loop; owns all windows and the notification state.
//! - D-Bus thread: serves org.freedesktop.Notifications via zbus's blocking
//!   API (zbus runs its own internal executor thread, so method calls are
//!   dispatched while we sit in the GTK loop).
//! - D-Bus -> GTK: an async-channel; a task spawned on the glib main context
//!   consumes it. GTK -> D-Bus: a cloned blocking connection.

mod daemon;
mod dbus;
mod window;
mod x11;

use gtk4 as gtk;

fn main() -> std::process::ExitCode {
    env_logger::init();

    if let Err(e) = gtk::init() {
        eprintln!("dunst-in-gtk: cannot initialize GTK (is DISPLAY set?): {e}");
        return std::process::ExitCode::from(1);
    }

    // GTK -> D-Bus signal emission needs the connection on this thread.
    let conn = match zbus::blocking::Connection::session() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("dunst-in-gtk: cannot connect to session bus: {e}");
            return std::process::ExitCode::from(1);
        }
    };
    daemon::init(conn);

    // D-Bus -> GTK event channel.
    let (tx, rx) = async_channel::unbounded::<dbus::DbusEvent>();
    std::thread::spawn(move || dbus::serve(tx));

    let ctx = glib::MainContext::default();
    ctx.spawn_local(async move {
        while let Ok(event) = rx.recv().await {
            daemon::handle(event);
        }
        log::warn!("D-Bus event channel closed, no more notifications");
    });

    // Graceful shutdown on SIGINT/SIGTERM (glib 0.22 removed unix_signal_add,
    // so poll a signal-hook flag).
    let main_loop = glib::MainLoop::new(None, false);
    {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;
        let quit_flag = Arc::new(AtomicBool::new(false));
        for sig in [signal_hook::consts::SIGINT, signal_hook::consts::SIGTERM] {
            if let Err(e) = signal_hook::flag::register(sig, Arc::clone(&quit_flag)) {
                log::warn!("cannot register signal handler: {e}");
            }
        }
        let main_loop = main_loop.clone();
        glib::timeout_add_local(std::time::Duration::from_millis(200), move || {
            if quit_flag.load(Ordering::Relaxed) {
                log::info!("signal received, quitting");
                main_loop.quit();
                glib::ControlFlow::Break
            } else {
                glib::ControlFlow::Continue
            }
        });
    }

    main_loop.run();
    log::info!("dunst-in-gtk exiting");
    std::process::ExitCode::SUCCESS
}
