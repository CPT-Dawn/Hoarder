//! Vendored Hyprland protocols (hyprwm/hyprland-protocols v0.8.0).
//!
//! `hyprland_toplevel_mapping_v1` gives the Hyprland window address of an
//! `ext_foreign_toplevel_handle_v1`, tying the toplevel list to `j/clients`.

#![allow(missing_debug_implementations, clippy::all)]

pub mod toplevel_mapping {
    use wayland_client;
    use wayland_protocols::ext::foreign_toplevel_list::v1::client::*;
    use wayland_protocols_wlr::foreign_toplevel::v1::client::*;

    pub mod __interfaces {
        use wayland_client::backend as wayland_backend;
        use wayland_protocols::ext::foreign_toplevel_list::v1::client::__interfaces::*;
        use wayland_protocols_wlr::foreign_toplevel::v1::client::__interfaces::*;
        wayland_scanner::generate_interfaces!("protocols/hyprland-toplevel-mapping-v1.xml");
    }
    use self::__interfaces::*;

    wayland_scanner::generate_client_code!("protocols/hyprland-toplevel-mapping-v1.xml");
}
