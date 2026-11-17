// Lattice's native whiteboard: quiet.md's GPUI port of Excalidraw
// (https://github.com/s1dashu/quiet-md, MIT, Copyright (c) 2026 s1dashu;
// Excalidraw https://github.com/excalidraw/excalidraw @ 3a317b5, MIT,
// Copyright (c) 2020 Excalidraw), adapted to Lattice's host. See
// THIRD_PARTY_NOTICES.md.

//! The `.excalidraw` whiteboard as a GPUI view (`board::BoardView`), over
//! `lattice-board-core`'s model. The host (`lattice-gpui`'s structured
//! documents) owns the file's text: it hands the view the text, takes the
//! serialized scene back when the view says it changed, and keeps the
//! reader's camera.

pub mod assets;
pub mod board;
pub mod text_field;

pub use lattice_board_core::whiteboard;

/// Registers the board's fonts (once, at startup).
pub fn load_fonts(cx: &gpui::App) {
    board::fonts::load(cx);
}

/// Binds the board's keys (with the app's, whenever those are bound again).
pub fn bind_keys(cx: &mut gpui::App) {
    board::bind_keys(cx);
    text_field::bind_keys(cx);
}

/// The look the board draws in, as the host's theme has it.
pub mod theme {
    use gpui::{Font, SharedString, font};
    use std::cell::{Cell, RefCell};

    thread_local! {
        static DARK: Cell<bool> = const { Cell::new(false) };
        static UI: RefCell<SharedString> = RefCell::new(SharedString::new_static("Inter"));
    }

    /// The board is drawn in the dark theme (Excalidraw's `theme === "dark"`).
    pub fn dark() -> bool { DARK.with(Cell::get) }

    pub(crate) fn set_dark(dark: bool) { DARK.with(|d| d.set(dark)); }

    /// The interface's family (the chrome's labels).
    pub fn ui_family() -> SharedString { UI.with(|u| u.borrow().clone()) }

    pub(crate) fn set_ui_family(family: SharedString) { UI.with(|u| *u.borrow_mut() = family); }

    pub fn ui_font() -> Font { font(ui_family()) }

    /// The chrome's monospace family (key hints, the hex field).
    pub const MONO_FONT: &str = "Ioskeley Mono";
}

/// The interface language: Excalidraw's English and Simplified Chinese.
pub mod settings {
    /// `zh` in Chinese, `en` otherwise.
    pub fn tr(zh: &'static str, en: &'static str) -> &'static str {
        if lattice_i18n::locale().is_chinese() { zh } else { en }
    }
}
