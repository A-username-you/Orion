use gtk::{glib, prelude::*};

fn main() -> glib::Result<()> {
    let app = gtk::Application::new(Some("org.test"))?;
    app.connect_activate(|app| {
        let window = gtk::ApplicationWindow::new(app);
        window.set_title(Some("Test"));
        window.set_default_size(200, 200);
        window.present();
    });
    app.run()
}
