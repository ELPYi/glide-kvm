//! Small reusable UI pieces. Each is a plain function taking `&mut Ui`.

mod card;
mod form;
mod layout_map;
mod pill;
mod this_pc;

pub use card::{card, card_with};
pub use form::{field, hint, section_title};
pub use layout_map::{layout_map, MapDevice};
pub use pill::pill;
pub use this_pc::this_pc_card;
