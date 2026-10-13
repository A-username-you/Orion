use gtk3::{glib, prelude::*};

fn main() -> glib::Result<()> {
    let app = gtk3::Application::builder().application_id("org.test.gtk3").build();
    app.connect_activate(| TrackingApp, _app| {
        let window = gtk3::ApplicationWindow::new(app);
        window.set_title(Some("GTK3 Test"));
        window.set_default_size(200, 200);
        window.present();
    });
    app.run()
}
