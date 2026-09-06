//! Small GTK installer: only the GTK runtime already shipped by Omarchy is needed.
//! GTK objects stay on the main thread; package/download work runs on a worker.
use std::ffi::{CString, c_char, c_int, c_void};
use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};

type Widget = *mut c_void;
#[link(name = "libgtk-3.so.0", kind = "dylib", modifiers = "+verbatim")]
unsafe extern "C" {
    fn gtk_init_check(argc: *mut c_int, argv: *mut *mut *mut c_char) -> c_int;
    fn gtk_window_new(kind: c_int) -> Widget;
    fn gtk_window_set_title(window: Widget, title: *const c_char);
    fn gtk_window_set_default_size(window: Widget, width: c_int, height: c_int);
    fn gtk_container_set_border_width(container: Widget, width: u32);
    fn gtk_container_add(container: Widget, child: Widget);
    fn gtk_box_new(orientation: c_int, spacing: c_int) -> Widget;
    fn gtk_box_pack_start(
        container: Widget,
        child: Widget,
        expand: c_int,
        fill: c_int,
        padding: u32,
    );
    fn gtk_label_new(text: *const c_char) -> Widget;
    fn gtk_label_set_text(label: Widget, text: *const c_char);
    fn gtk_label_set_line_wrap(label: Widget, wrap: c_int);
    fn gtk_label_set_xalign(label: Widget, alignment: f32);
    fn gtk_button_new_with_label(text: *const c_char) -> Widget;
    fn gtk_button_set_label(button: Widget, text: *const c_char);
    fn gtk_widget_set_sensitive(widget: Widget, sensitive: c_int);
    fn gtk_widget_show_all(widget: Widget);
    fn gtk_widget_get_window(widget: Widget) -> Widget;
    fn gtk_widget_get_allocated_width(widget: Widget) -> c_int;
    fn gtk_widget_get_allocated_height(widget: Widget) -> c_int;
    fn gtk_progress_bar_new() -> Widget;
    fn gtk_progress_bar_pulse(progress: Widget);
    fn gtk_progress_bar_set_fraction(progress: Widget, fraction: f64);
    fn gtk_main();
    fn gtk_main_quit();
}
#[link(name = "libgdk-3.so.0", kind = "dylib", modifiers = "+verbatim")]
unsafe extern "C" {
    fn gdk_pixbuf_get_from_window(
        window: Widget,
        x: c_int,
        y: c_int,
        width: c_int,
        height: c_int,
    ) -> Widget;
}
#[link(
    name = "libgdk_pixbuf-2.0.so.0",
    kind = "dylib",
    modifiers = "+verbatim"
)]
unsafe extern "C" {
    fn gdk_pixbuf_savev(
        pixbuf: Widget,
        filename: *const c_char,
        kind: *const c_char,
        keys: Widget,
        values: Widget,
        error: Widget,
    ) -> c_int;
}
#[link(name = "libgobject-2.0.so.0", kind = "dylib", modifiers = "+verbatim")]
unsafe extern "C" {
    fn g_signal_connect_data(
        instance: Widget,
        signal: *const c_char,
        callback: unsafe extern "C" fn(),
        data: Widget,
        destroy: Widget,
        flags: u32,
    ) -> u64;
}
#[link(name = "libglib-2.0.so.0", kind = "dylib", modifiers = "+verbatim")]
unsafe extern "C" {
    fn g_timeout_add(
        interval: u32,
        callback: unsafe extern "C" fn(Widget) -> c_int,
        data: Widget,
    ) -> u32;
}

enum Event {
    Status(String),
    Finished(Result<(), String>),
}
struct Ui {
    window: Widget,
    capture: Option<String>,
    ticks: u32,
    label: Widget,
    button: Widget,
    progress: Widget,
    receiver: Receiver<Event>,
    sender: Sender<Event>,
    busy: bool,
    installed: bool,
}
fn c(text: &str) -> CString {
    CString::new(text.replace('\0', "")).expect("NUL removed")
}
fn launcher() -> Result<PathBuf, String> {
    std::env::var_os("HOME")
        .map(|home| PathBuf::from(home).join(".local/bin/omasheets"))
        .ok_or("Your home directory is unavailable.".into())
}

// Capture bounded diagnostics while draining the pipe to avoid child deadlocks.
fn run(command: &mut Command) -> Result<(), String> {
    let mut child = command
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    let mut pipe = child
        .stderr
        .take()
        .ok_or("Unable to read installer output")?;
    let mut tail = Vec::new();
    let mut bytes = [0; 4096];
    loop {
        let count = pipe.read(&mut bytes).map_err(|e| e.to_string())?;
        if count == 0 {
            break;
        }
        tail.extend_from_slice(&bytes[..count]);
        if tail.len() > 6000 {
            tail.drain(..tail.len() - 6000);
        }
    }
    if child.wait().map_err(|e| e.to_string())?.success() {
        Ok(())
    } else {
        Err(format!(
            "Installation did not finish. Your existing workbooks are preserved.\n\n{}",
            String::from_utf8_lossy(&tail)
        ))
    }
}

fn install(events: &Sender<Event>) -> Result<(), String> {
    if !std::path::Path::new("/etc/arch-release").is_file() {
        return Err("This installer is for Omarchy on Linux x86_64 (Arch Linux).".into());
    }
    // All package operations are fixed arguments to the system package manager.
    // The button explains this and Polkit asks for authentication if needed.
    let packages = [
        "curl",
        "jq",
        "git",
        "python",
        "gtk3",
        "libreoffice-fresh",
        "bubblewrap",
        "qt6-base",
        "qt6-declarative",
        "qt6-wayland",
    ];
    let missing = Command::new("/usr/bin/pacman")
        .arg("-Q")
        .args(packages)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|e| e.to_string())?;
    if !missing.success() {
        let _ = events.send(Event::Status("Installing required system packages. Authorise the system prompt to continue. This may take a few minutes.".into()));
        run(Command::new("/usr/bin/pkexec")
            .args(["/usr/bin/pacman", "-Syu", "--needed", "--noconfirm"])
            .args(packages))?;
    }
    let _ = events.send(Event::Status("Downloading and verifying OmaSheets…".into()));
    // Download before execution, with a strict bound and HTTPS-only redirects.
    let output = Command::new("/usr/bin/curl")
        .args([
            "--fail",
            "--silent",
            "--show-error",
            "--location",
            "--proto",
            "=https",
            "--proto-redir",
            "=https",
            "--max-time",
            "60",
            "--max-filesize",
            "65536",
            "https://raw.githubusercontent.com/tcballard/OmaSheets/main/bin/omasheets-install",
        ])
        .output()
        .map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err(format!(
            "Could not download the installer. Check your connection and retry.\n{}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    // The script is the command; no local paths or user text are interpolated into it.
    let script = String::from_utf8(output.stdout).map_err(|e| e.to_string())?;
    run(Command::new("/bin/bash").arg("-c").arg(script))
}

unsafe extern "C" fn clicked(_: Widget, data: Widget) {
    // SAFETY: data is the boxed Ui, alive for the entire GTK main loop.
    let ui = unsafe { &mut *data.cast::<Ui>() };
    if ui.busy {
        return;
    }
    if ui.installed {
        match launcher().and_then(|path| {
            Command::new(path)
                .spawn()
                .map(|_| ())
                .map_err(|e| e.to_string())
        }) {
            Ok(()) => unsafe { gtk_main_quit() },
            Err(error) => unsafe { gtk_label_set_text(ui.label, c(&error).as_ptr()) },
        }
        return;
    }
    ui.busy = true;
    unsafe {
        gtk_widget_set_sensitive(ui.button, 0);
        gtk_label_set_text(ui.label, c("Preparing installation…").as_ptr());
    }
    let sender = ui.sender.clone();
    std::thread::spawn(move || {
        let result = install(&sender);
        let _ = sender.send(Event::Finished(result));
    });
}
unsafe extern "C" fn close(_: Widget, _: Widget, data: Widget) -> c_int {
    let ui = unsafe { &*data.cast::<Ui>() };
    if ui.busy {
        return 1;
    }
    unsafe { gtk_main_quit() };
    0
}
unsafe extern "C" fn poll(data: Widget) -> c_int {
    let ui = unsafe { &mut *data.cast::<Ui>() };
    ui.ticks = ui.ticks.saturating_add(1);
    if ui.ticks == 5
        && let Some(path) = &ui.capture
    {
        unsafe {
            let pixbuf = gdk_pixbuf_get_from_window(
                gtk_widget_get_window(ui.window),
                0,
                0,
                gtk_widget_get_allocated_width(ui.window),
                gtk_widget_get_allocated_height(ui.window),
            );
            if pixbuf.is_null()
                || gdk_pixbuf_savev(
                    pixbuf,
                    c(path).as_ptr(),
                    c("png").as_ptr(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                ) == 0
            {
                std::process::exit(1);
            }
            gtk_main_quit();
        }
    }
    if ui.busy {
        unsafe { gtk_progress_bar_pulse(ui.progress) };
    }
    while let Ok(event) = ui.receiver.try_recv() {
        match event {
            Event::Status(message) => unsafe { gtk_label_set_text(ui.label, c(&message).as_ptr()) },
            Event::Finished(result) => {
                ui.busy = false;
                ui.installed = result.is_ok();
                let message = result.err().unwrap_or_else(|| "OmaSheets is ready. You can also find it in your application launcher.\n\nStart with a blank spreadsheet or try the guided example.".into());
                unsafe {
                    gtk_label_set_text(ui.label, c(&message).as_ptr());
                    gtk_button_set_label(
                        ui.button,
                        c(if ui.installed {
                            "Open OmaSheets"
                        } else {
                            "Retry installation"
                        })
                        .as_ptr(),
                    );
                    gtk_widget_set_sensitive(ui.button, 1);
                    gtk_progress_bar_set_fraction(
                        ui.progress,
                        if ui.installed { 1.0 } else { 0.0 },
                    );
                }
            }
        }
    }
    1
}

fn main() {
    if std::env::args().any(|arg| arg == "--provenance") {
        println!(
            "{{\"source_commit\":\"{}\",\"source_sha256\":\"{}\"}}",
            option_env!("OMASHEETS_SOURCE_COMMIT").unwrap_or("development"),
            option_env!("OMASHEETS_SOURCE_SHA256").unwrap_or("development")
        );
        return;
    }
    // SAFETY: GTK calls and UI pointer access are confined to this main thread.
    unsafe {
        if gtk_init_check(std::ptr::null_mut(), std::ptr::null_mut()) == 0 {
            eprintln!("Open this installer from your desktop session.");
            std::process::exit(1);
        }
        let window = gtk_window_new(0);
        gtk_window_set_title(window, c("OmaSheets Setup").as_ptr());
        gtk_window_set_default_size(window, 560, 360);
        gtk_container_set_border_width(window, 28);
        let column = gtk_box_new(1, 20);
        gtk_container_add(window, column);
        let title = gtk_label_new(c("Welcome to OmaSheets").as_ptr());
        gtk_box_pack_start(column, title, 0, 0, 0);
        let label = gtk_label_new(c("Install the latest development build of OmaSheets. Your spreadsheets stay on this computer.\n\nClose any OmaSheets windows first. Setup downloads the app and, if needed, asks you to authorise system package installation and updates.\n\nThis is a development preview; some Excel features are still unsupported.").as_ptr());
        gtk_label_set_line_wrap(label, 1);
        gtk_label_set_xalign(label, 0.0);
        gtk_box_pack_start(column, label, 1, 1, 0);
        let progress = gtk_progress_bar_new();
        gtk_box_pack_start(column, progress, 0, 0, 0);
        let button = gtk_button_new_with_label(c("Install / update OmaSheets").as_ptr());
        gtk_box_pack_start(column, button, 0, 0, 0);
        let (sender, receiver) = mpsc::channel();
        let data = Box::into_raw(Box::new(Ui {
            window,
            capture: std::env::args().skip_while(|arg| arg != "--capture").nth(1),
            ticks: 0,
            label,
            button,
            progress,
            sender,
            receiver,
            busy: false,
            installed: false,
        }))
        .cast();
        g_signal_connect_data(
            button,
            c("clicked").as_ptr(),
            std::mem::transmute::<unsafe extern "C" fn(Widget, Widget), unsafe extern "C" fn()>(
                clicked,
            ),
            data,
            std::ptr::null_mut(),
            0,
        );
        g_signal_connect_data(
            window,
            c("delete-event").as_ptr(),
            std::mem::transmute::<
                unsafe extern "C" fn(Widget, Widget, Widget) -> c_int,
                unsafe extern "C" fn(),
            >(close),
            data,
            std::ptr::null_mut(),
            0,
        );
        g_timeout_add(100, poll, data);
        gtk_widget_show_all(window);
        gtk_main();
        // Process exits immediately; callbacks are no longer dispatched.
        drop(Box::from_raw(data.cast::<Ui>()));
    }
}
