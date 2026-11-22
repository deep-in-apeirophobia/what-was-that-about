//! Font-DPI guard for WebKitGTK.
//!
//! WebKitGTK (GTK3) zooms every page by `font DPI / 96`, reading the DPI from
//! `gdk_screen_get_resolution()`. GDK reports -1 when it can't learn the desktop's
//! DPI: on Wayland it only reads it from the GNOME GSettings schema, which unwrapped
//! builds (a NixOS dev shell) and minimal desktops don't have. WebKit turns that -1
//! into a negative zoom (`devicePixelRatio` -0.02, negative `innerWidth`) and the
//! layout collapses to a word per line. Supplying a DPI before the first webview
//! exists avoids it.

use gtk::prelude::*;

/// DPI to apply when the screen's is unusable: GTK's own `gtk-xft-dpi` (1024ths of
/// a dot per inch, -1 when unset) if it has one, else the CSS reference 96.
fn replacement_dpi(screen_dpi: f64, xft_dpi: i32) -> Option<f64> {
    if screen_dpi > 0.0 {
        return None;
    }
    Some(if xft_dpi > 0 {
        f64::from(xft_dpi) / 1024.0
    } else {
        96.0
    })
}

/// Must run on the main thread before Tauri builds its windows. Returns the DPI it
/// set (for logging once the logger is up), or `None` if nothing needed fixing.
pub fn ensure_font_dpi() -> Option<f64> {
    gtk::init().ok()?;
    // Settings first: creating them recomputes the screen resolution from gtk-xft-dpi.
    let settings = gtk::Settings::default()?;
    let screen = gtk::gdk::Screen::default()?;
    let xft = settings.gtk_xft_dpi();
    let dpi = replacement_dpi(screen.resolution(), xft)?;
    if xft <= 0 {
        settings.set_gtk_xft_dpi((dpi * 1024.0).round() as i32);
    }
    screen.set_resolution(dpi);
    Some(dpi)
}

#[cfg(test)]
mod tests {
    use super::replacement_dpi;

    #[test]
    fn only_replaces_unknown_dpi() {
        assert_eq!(replacement_dpi(96.0, -1), None);
        assert_eq!(replacement_dpi(120.0, 98304), None);
        assert_eq!(replacement_dpi(-1.0, -1), Some(96.0));
        assert_eq!(replacement_dpi(-1.0, 0), Some(96.0));
        assert_eq!(replacement_dpi(-1.0, 122880), Some(120.0));
    }
}
