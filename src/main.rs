mod config;
mod fileops;
mod i18n;
mod markdown;
mod note;
mod panels;
mod theme;
mod tree;
mod vault;
mod window;

use adw::prelude::*;
use gtk::{gio, glib};
use std::cell::RefCell;
use std::rc::Rc;

const APP_ID: &str = "io.github.wioletowa.gtkdown";

fn main() -> glib::ExitCode {
    let config = config::Config::load();
    i18n::init(&config.lang);

    let app = adw::Application::builder()
        .application_id(APP_ID)
        .flags(gio::ApplicationFlags::HANDLES_OPEN)
        .build();

    let state: Rc<RefCell<Option<Rc<window::App>>>> = Rc::default();
    let config = Rc::new(RefCell::new(Some(config)));

    let get = {
        let state = state.clone();
        move |app: &adw::Application| -> Rc<window::App> {
            if let Some(w) = &*state.borrow() {
                return w.clone();
            }
            let w = window::App::new(app, config.borrow_mut().take().unwrap_or_default());
            *state.borrow_mut() = Some(w.clone());
            w
        }
    };

    let g = get.clone();
    app.connect_activate(move |app| {
        let fresh = !app.windows().iter().any(|w| w.is_visible());
        let w = g(app);
        if fresh {
            w.restore();
        }
        w.present();
        #[cfg(debug_assertions)]
        if let Ok(script) = std::env::var("GTKDOWN_TEST") {
            w.run_script(script);
        }
    });

    app.connect_open(move |app, files, _| {
        let w = get(app);
        for f in files {
            let Some(p) = f.path() else { continue };
            if p.is_dir() {
                w.open_vault(p);
            } else {
                w.open_vault(window::find_vault_root(&p));
                w.open_file(p, false);
            }
        }
        w.ensure_tab();
        w.present();
        #[cfg(debug_assertions)]
        if let Ok(script) = std::env::var("GTKDOWN_TEST") {
            w.run_script(script);
        }
    });

    app.run()
}
