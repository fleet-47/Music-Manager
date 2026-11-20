//! The `sdl` terminal app: the screen ([`screen`]), the app state it draws ([`app`]), the five
//! functions' dialogs ([`dialogs`]) and plain-text helpers ([`fmt`]). Forms are `sdl_form`; the
//! task layer is `sdl_tasks`.

pub mod app;
pub mod dialogs;
pub mod fmt;
pub mod screen;
