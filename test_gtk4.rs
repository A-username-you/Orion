use gtk4::{glib, Application, ApplicationWindow, prelude::*};

fn main() -> glib::Result<()> {
    let app = Application::builder().application_id("org.test").build();
    app.connect_activate(|app| {
        let window = ApplicationWindow::new(app);
        window.set_title(Some("Test"));
        window.set_decorated(false);
        window.set_skip_taskbar_hint(true);
        window.set_skip_pager_hint(true);
        window.set_keep_above(true);
        window.present();
    });
    app.run()
}