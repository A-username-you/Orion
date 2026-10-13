use gtk::{glib, prelude::*};
use gtk::ApplicationFlags;
use std::fs;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let app = gtk::Application::new(Some("org.orion.indicator"), ApplicationFlags::empty())?;
    app.connect_activate(move |app| {
        let window = gtk::ApplicationWindow::new(app);
        window.set_title(Some("Orion"));
        window.set_decorated(false);
        window.set_skip_taskbar_hint(true);
        window.set_skip_pager_hint(true);
        window.set_keep_above(true);
        window.set_default_size(24, 24);
        window.move_(1800, 10); // top‑right corner

        let vbox = gtk::Box::new(gtk::Orientation::Vertical, 2);
        vbox.set_margin_start(2);
        vbox.set_margin_end(2);
        vbox.set_margin_top(2);
        vbox.set_margin_bottom(2);

        let state_label = gtk::Label::new(Some("●"));
        state_label.set_halign(gtk::Align::Center);
        vbox.add(&state_label);

        window.add(&vbox);

        // Update label from /tmp/orion.state every second
        let state_label_clone = state_label.clone();
        glib::timeout_add_seconds_local(1, move || {
            let state = fs::read_to_string("/tmp/orion.state")
                .unwrap_or_else(|_| "idle".to_string())
                .trim()
                .to_string();
            let text = match state.as_str() {
                "idle" => "●",
                "listening" => "●",
                "thinking" => "●",
                "speaking" => "●",
                _ => "●",
            };
            state_label_clone.set_label(&text);
            glib::Continue(true)
        });

        // Left click → show state, right click → quit
        window.connect_button_press_event(move |_win, event| {
            if let Some(button_event) = event.downcast_ref::<gdk::EventButton>() {
                match button_event.get_button() {
                    1 => { // Left click
                        let state = fs::read_to_string("/tmp/orion.state")
                            .unwrap_or_else(|_| "idle".to_string())
                            .trim()
                            .to_string();
                        let dialog = gtk::MessageDialog::new(
                            Some(&window),
                            gtk::DialogFlags::MODAL,
                            gtk::MessageType::Info,
                            gtk::ButtonsType::Ok,
                            &format!("Orion State: {}", state),
                        );
                        dialog.set_title(Some("Orion State"));
                        dialog.connect_response(move |d, _| {
                            d.close();
                        });
                        dialog.show_all();
                        gtk::Inhibit(false)
                    }
                    3 => { // Right click
                        window.close();
                        gtk::Inhibit(false)
                    }
                    _ => gtk::Inhibit(false),
                }
            } else {
                gtk::Inhibit(false)
            }
        });

        window.show_all();
    });

    app.run();
    Ok(())
}