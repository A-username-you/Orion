use gtk4::glib::{self, Continue};
use gtk4::prelude::*;
use gtk4::{Application, ApplicationWindow, Label};

fn main() -> gtk4::glib::Result<()> {
    let app = Application::builder().application_id("org.example.test").build();

    app.connect_activate(|app| {
        let window = ApplicationWindow::new(app);
        window.set_title("Hello World");
        window.set_default_size(300, 200);
        let label = Label::new(Some("Hello from GTK4"));
        window.set_child(Some(&label));
        window.present();
    });

    app.run();
    Ok(())
}
