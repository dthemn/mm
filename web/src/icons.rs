//! Line icons drawn for this app, as inline SVG. They follow the text color, so both themes work.

use leptos::prelude::*;
use mm_core::model::*;
use uuid::Uuid;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Icon {
    Home,
    Stats,
    Camera,
    Plus,
    Settings,
    Back,
    Wallet,
    Download,
    Cart,
    Food,
    Health,
    Transport,
    Clothing,
    Fun,
    Repeat,
    Receipt,
    Transfer,
    Close,
    Flash,
    Gallery,
    Pencil,
    Calendar,
}

fn paths(i: Icon) -> &'static [&'static str] {
    match i {
        Icon::Home => &["M3 11 12 4l9 7", "M5.5 9.5V20h13V9.5", "M10 20v-5h4v5"],
        Icon::Stats => &["M5 20V12", "M12 20V5", "M19 20v-9"],
        Icon::Camera => &["M4 8h3l2-3h6l2 3h3v11H4z", "M15.5 13a3.5 3.5 0 1 1-7 0 3.5 3.5 0 0 1 7 0z"],
        Icon::Plus => &["M12 5v14", "M5 12h14"],
        Icon::Settings => &[
            "M15 12a3 3 0 1 1-6 0 3 3 0 0 1 6 0z",
            "M12 2.5v3", "M12 18.5v3", "M2.5 12h3", "M18.5 12h3",
            "M5.3 5.3l2.1 2.1", "M16.6 16.6l2.1 2.1", "M5.3 18.7l2.1-2.1", "M16.6 7.4l2.1-2.1",
        ],
        Icon::Back => &["M20 12H5", "M11 6l-6 6 6 6"],
        Icon::Wallet => &["M4 7h15v12H4z", "M4 7l11-3v3", "M15 13h4"],
        Icon::Download => &["M12 4v10", "M8 10l4 4 4-4", "M4 15v4h16v-4"],
        Icon::Cart => &["M3 5h2.5l2 10h10l2-7H7", "M10 19.5h.01", "M16 19.5h.01"],
        Icon::Food => &["M7 3v8", "M4.5 3v5a2.5 2.5 0 0 0 5 0V3", "M7 11v10", "M17 21V3c-2 1.5-3 4-3 7v3h3"],
        Icon::Health => &["M4 4h16v16H4z", "M12 8v8", "M8 12h8"],
        Icon::Transport => &["M5 4h14v12H5z", "M5 11h14", "M7.5 16v3", "M16.5 16v3", "M8 13.5h.01", "M16 13.5h.01"],
        Icon::Clothing => &["M8 4l4 3 4-3 5 4-3 3v9H6v-9L3 8z"],
        Icon::Fun => &["M4 7h16v3a2 2 0 0 0 0 4v3H4v-3a2 2 0 0 0 0-4z", "M14 7v10"],
        Icon::Repeat => &["M4 11V9a3 3 0 0 1 3-3h12", "M16 3l3 3-3 3", "M20 13v2a3 3 0 0 1-3 3H5", "M8 21l-3-3 3-3"],
        Icon::Receipt => &["M6 3h12v18l-2-1.5-2 1.5-2-1.5-2 1.5-2-1.5L6 21z", "M9 8h6", "M9 12h6", "M9 16h3"],
        Icon::Transfer => &["M4 8h14", "M15 5l3 3-3 3", "M20 16H6", "M9 13l-3 3 3 3"],
        Icon::Close => &["M6 6l12 12", "M18 6L6 18"],
        Icon::Flash => &["M13 2L5 14h6l-1 8 8-12h-6z"],
        Icon::Gallery => &["M4 5h16v14H4z", "M4 16l5-5 4 4 3-3 4 4", "M15.5 9h.01"],
        Icon::Pencil => &["M4 20h4L19 9l-4-4L4 16z", "M13.5 6.5l4 4"],
        Icon::Calendar => &["M4 6h16v14H4z", "M4 10h16", "M8 3v5", "M16 3v5"],
    }
}

pub fn icon(i: Icon) -> impl IntoView {
    let d = paths(i).iter().map(|p| view! { <path d=*p/> }).collect_view();
    view! { <svg class="icon" viewBox="0 0 24 24" aria-hidden="true">{d}</svg> }
}

/// The picture for an operation: its kind first, then its category.
pub fn for_tx(t: &Transaction, category: Uuid) -> Icon {
    match t.kind {
        TxKind::Income => Icon::Download,
        TxKind::Transfer => Icon::Transfer,
        TxKind::Expense => match category {
            c if c == CAT_FOOD => Icon::Cart,
            c if c == CAT_HOUSEHOLD => Icon::Home,
            c if c == CAT_TRANSPORT => Icon::Transport,
            c if c == CAT_HEALTH => Icon::Health,
            c if c == CAT_CLOTHING => Icon::Clothing,
            c if c == CAT_ENTERTAINMENT => Icon::Fun,
            c if c == CAT_CAFES => Icon::Food,
            c if c == CAT_SUBSCRIPTIONS => Icon::Repeat,
            _ => Icon::Receipt,
        },
    }
}
