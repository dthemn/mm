//! The client. This is the only place data exists in plaintext.
#![forbid(unsafe_code)]

mod api;
mod app;
mod data;
mod device;
mod finance;
mod flows;
mod icons;
mod import;
mod landing;
mod scan;
mod store;
mod webauthn;

fn main() {
    console_error_panic_hook::set_once();
    leptos::mount::mount_to_body(app::App);
}
