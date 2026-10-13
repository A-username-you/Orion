use anyhow::Result;
use gtk4::gdk::{self, ModifierType, Inhibit, RGBA};
use gtk4::glib::{self, Continue};
use gtk4::prelude::*;
use gtk4::{Application, ApplicationWindow, DrawingArea, Menu, MenuItem};
use gdk_pixbuf::{InterpType, Pixbuf};
use std::env;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

struct State {
    current: String,
}

fn main() -> Result<()> {
    let app = Application::builder().application_id("org.orion.pet").build();

    let state = Arc::new(Mutex::new(State {
        current: "idle".to_string(),
    }));
    let state_clone = state.clone();

    app.connect_activate(move |app| {
        let state = state_clone.clone();

        // Load image
        let image_path = env::var("ORION_PET_IMAGE").unwrap_or_else(|_| "orion.png".to_string());
        let pixbuf = load_or_fallback_image(&image_path, 64, 64)
            .unwrap_or_else(|_| create_fallback_image(64, 64));
        let pixbuf_clone = pixbuf.clone();

        let window = ApplicationWindow::new(app);
        window.set_decorated(false);
        window.set_skip_taskbar_hint(true);
        window.set_skip_pager_hint(true);
        window.set_app_paintable(true);
        // Try to set RGBA visual for transparency if compositor supports
        if let Some(screen) = window.screen() {
            if let Some(visual) = screen.rgba_visual() {
                window.set_visual(&visual);
            }
        }
        window.set_default_size(64, 64);
        window.set_resizable(false);

        // Enable mouse events for dragging
        window.add_events(gdk::EventMask::BUTTON_PRESS_MASK | gdk::EventMask::POINTER_MOTION_MASK);
        let window_clone = window.clone();
        window.connect_button_press_event(move |_, _, button| {
            if button.get_button() == 1 {
                Inhibit(false)
            } else {
                Inhibit(true)
            }
        });
        let window_clone2 = window.clone();
        window.connect_motion_notify_event(move |_, _, x, y| {
            let state = window.get_modifier_state();
            if state.contains(ModifierType::BUTTON1_MASK) {
                window.r#move(x as i32 - 32, y as i32 - 32);
            }
            Inhibit(false)
        });

        let drawing = DrawingArea::new();
        drawing.set_size_request(64, 64);
        let state_ref = state.clone();
        let pixbuf_ref = pixbuf_clone;
        drawing.connect_draw(move |_, cr, width, height| {
            let state = state_ref.lock().unwrap();
            // Calculate state based on system time (each state lasts 2 seconds)
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("Time went backwards")
                .as_secs();
            let state_str = match now % 8 {
                0..=1 => "idle",
                2..=3 => "listening",
                4..=5 => "thinking",
                6..=7 => "speaking",
                _ => "idle",
            };
            let overlay_color = match state_str {
                "idle" => RGBA::new(0.5, 0.5, 0.5, 0.3), // semi-transparent gray
                "listening" => RGBA::new(0.2, 0.6, 1.0, 0.3), // blue
                "thinking" => RGBA::new(1.0, 0.8, 0.0, 0.3), // yellow
                "speaking" => RGBA::new(0.2, 0.8, 0.2, 0.3), // green
                _ => RGBA::new(0.5, 0.5, 0.5, 0.3),
            };

            // Clear with transparent
            cr.set_source_rgba(0.0, 0.0, 0.0, 0.0);
            cr.paint().unwrap();

            // Draw the image (scaled to fit the window)
            gdk::cairo_set_source_pixbuf(cr, &pixbuf_ref, 0.0, 0.0);
            cr.paint().unwrap();

            // Draw overlay color
            cr.set_source_rgba(
                overlay_color.red(),
                overlay_color.green(),
                overlay_color.blue(),
                overlay_color.alpha(),
            );
            cr.rectangle(0.0, 0.0, width as f64, height as f64);
            cr.fill().unwrap();

            Inhibit(false)
        });

        window.add(&drawing);

        // Right-click menu (context menu)
        let menu = Menu::new();
        {
            let state_for_show = state.clone();
            let show_state = MenuItem::with_label("Show State");
            show_state.connect_activate(move |_| {
                let state = state_for_show.lock().unwrap();
                let dialog = gtk4::MessageDialog::new(
                    Some(&window),
                    gtk4::DialogFlags::MODAL,
                    gtk4::MessageType::Info,
                    gtk4::ButtonsType::Ok,
                    &format!("Current state: {}", state.current),
                );
                dialog.set_title(Some("Orion State"));
                dialog.connect_response(move |d, _| {
                    d.close();
                    Inhibit(false)
                });
                dialog.show();
            });
            menu.append(&show_state);
        }
        let quit_item = MenuItem::with_label("Quit");
        quit_item.connect_activate(move |_| {
            window.close();
        });
        menu.append(&quit_item);
        menu.show_all();

        // Attach the menu to the window on right-click (button 3)
        let window_for_menu = window.clone();
        window.connect_button_release_event(move |_, _, button| {
            if button.get_button() == 3 { // right click
                menu.popup_at_pointer(None);
                Inhibit(false)
            } else {
                Inhibit(true)
            }
        });

        window.show_all();
    });

    app.run();
    Ok(())
}

fn load_or_fallback_image(path: &str, width: i32, height: i32) -> Result<Pixbuf> {
    let pb = Pixbuf::from_file(path)?;
    let pb = pb.scale_simple(width, height, InterpType::Bilinear)?;
    Ok(pb)
}

fn create_fallback_image(width: i32, height: i32) -> Pixbuf {
    let pb = Pixbuf::new(gdk_pixbuf::Colorspace::Rgb, true, 8, width, height).expect("Failed to create Pixbuf");
    // Fill with transparent
    pb.fill(0x00000000);
    // Draw a gray circle
    let radius = (width.min(height) as f64) * 0.4;
    let cx = width as f64 / 2.0;
    let cy = height as f64 / 2.0;
    for y in 0..height {
        for x in 0..width {
            let dx = (x as f64 - cx) / radius;
            let dy = (y as f64 - cy) / radius;
            if dx * dx + dy * dy <= 1.0 {
                // Inside circle: set to semi-transparent gray
                // Note: put_pixel expects (x, y, r, g, b, alpha) as u8, u8, u8, u8
                // We want gray: 128,128,128 and alpha 128 (0x80) for 50% transparency
                pb.put_pixel(
                    x as u32,
                    y as u32,
                    128,
                    128,
                    128,
                    128,
                );
            }
        }
    }
    pb
}