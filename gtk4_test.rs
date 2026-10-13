use gtk4::{glib, Application, ApplicationWindow, prelude::*};

fn main() -> glib::Result<()> {
    let app = Application::builder().application_id("org.test.gtk4").build();
    app.connect_activate(|app| {
        let window = ApplicationWindow::new(app);
        window.set_title(Some("GTK4 Test"));
        window.set_default_size(200, 200);
        window.present();
    });
    app.run()
}