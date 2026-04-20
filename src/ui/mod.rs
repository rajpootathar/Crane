//! UI rendering — panels, pane dispatch, chrome. No state mutation
//! beyond the `&mut App` parameter each render fn receives.

pub mod branch_picker;
pub mod explorer;
pub mod pane_view;
pub mod projects;
pub mod status;
pub mod top;
pub mod util;
