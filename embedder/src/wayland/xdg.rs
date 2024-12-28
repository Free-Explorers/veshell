use std::cell::RefCell;

use serde_json::json;
use smithay::{
    reexports::{
        wayland_protocols::xdg::shell::server::xdg_toplevel,
        wayland_server::{protocol::wl_seat::WlSeat, Resource},
    },
    utils::{Logical, Serial},
    wayland::{
        compositor::with_states,
        shell::xdg::{
            PopupSurface, PositionerState, ToplevelSurface, XdgPopupSurfaceData, XdgShellHandler,
            XdgShellState, XdgToplevelSurfaceData,
        },
    },
};

use crate::{
    flutter_engine::wayland_messages::MyPoint,
    state::State,
    wayland::wayland::{get_surface_id, WlSurfaceVeshellState},
    Backend,
};

impl<BackendData: Backend> XdgShellHandler for State<BackendData> {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg_shell_state
    }

    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        let surface_id = get_surface_id(surface.wl_surface());
        self.xdg_toplevels.insert(surface_id, surface.clone());

        surface.with_pending_state(|state| {
            state.states.set(xdg_toplevel::State::Activated);
        });

        let pid = {
            let client = surface.wl_surface().client().unwrap();

            let credentials = client.get_credentials(&self.display_handle).unwrap();

            credentials.pid
        };

        let platform_method_channel = &mut self.flutter_engine_mut().platform_method_channel;
        platform_method_channel.invoke_method(
            "new_toplevel",
            Some(Box::new(json!({
                "surfaceId": surface_id,
                "pid": pid,
            }))),
            None,
        );
    }

    fn new_popup(&mut self, surface: PopupSurface, positioner: PositionerState) {
        surface.with_pending_state(|state| {
            state.geometry = positioner.get_geometry();
            state.positioner = positioner;
        });

        let (surface_id, parent) = with_states(surface.wl_surface(), |surface_data| {
            let surface_id = surface_data
                .data_map
                .get::<RefCell<WlSurfaceVeshellState>>()
                .unwrap()
                .borrow()
                .surface_id;

            let parent = surface_data
                .data_map
                .get::<XdgPopupSurfaceData>()
                .unwrap()
                .lock()
                .unwrap()
                .parent
                .clone();

            (surface_id, parent)
        });

        self.xdg_popups.insert(surface_id, surface.clone());

        // TODO: Revise this unwrap.
        // Wayland states that popups without parents can exist but I don't know in what case.
        let parent = get_surface_id(&parent.unwrap());
        let position: MyPoint<i32, Logical> = positioner.get_geometry().loc.into();

        let platform_method_channel = &mut self.flutter_engine_mut().platform_method_channel;
        platform_method_channel.invoke_method(
            "new_popup",
            Some(Box::new(json!({
                "surfaceId": surface_id,
                "parent": parent,
                "position": position,
            }))),
            None,
        );
    }

    fn move_request(&mut self, surface: ToplevelSurface, _seat: WlSeat, _serial: Serial) {
        let surface_id = get_surface_id(surface.wl_surface());
        let platform_method_channel = &mut self.flutter_engine_mut().platform_method_channel;
        platform_method_channel.invoke_method(
            "interactive_move",
            Some(Box::new(json!({
                    "surfaceId": surface_id,
            }))),
            None,
        );
    }

    fn resize_request(
        &mut self,
        surface: ToplevelSurface,
        _seat: WlSeat,
        _serial: Serial,
        edges: xdg_toplevel::ResizeEdge,
    ) {
        let surface_id = get_surface_id(surface.wl_surface());
        let platform_method_channel = &mut self.flutter_engine_mut().platform_method_channel;
        platform_method_channel.invoke_method(
            "interactive_resize",
            Some(Box::new(json!({
                    "surfaceId": surface_id,
                    "edge": edges as i64,
            }))),
            None,
        );
    }

    fn grab(&mut self, _surface: PopupSurface, _seat: WlSeat, _serial: Serial) {
        // Handle popup grab here
    }

    fn reposition_request(
        &mut self,
        surface: PopupSurface,
        _positioner: PositionerState,
        token: u32,
    ) {
        surface.send_repositioned(token);
    }

    fn toplevel_destroyed(&mut self, surface: ToplevelSurface) {
        let surface_id = get_surface_id(surface.wl_surface());
        self.xdg_toplevels.remove(&surface_id);

        let platform_method_channel = &mut self.flutter_engine_mut().platform_method_channel;
        platform_method_channel.invoke_method(
            "destroy_toplevel",
            Some(Box::new(json!({
                "surfaceId": surface_id,
            }))),
            None,
        );
    }

    fn popup_destroyed(&mut self, surface: PopupSurface) {
        let surface_id = get_surface_id(surface.wl_surface());
        self.xdg_popups.remove(&surface_id);

        let platform_method_channel = &mut self.flutter_engine_mut().platform_method_channel;
        platform_method_channel.invoke_method(
            "destroy_popup",
            Some(Box::new(json!({
                "surfaceId": surface_id,
            }))),
            None,
        );
    }

    fn app_id_changed(&mut self, surface: ToplevelSurface) {
        let surface_id = get_surface_id(surface.wl_surface());

        let app_id = with_states(surface.wl_surface(), |surface_data| {
            surface_data
                .data_map
                .get::<XdgToplevelSurfaceData>()
                .unwrap()
                .lock()
                .unwrap()
                .app_id
                .clone()
        });

        let platform_method_channel = &mut self.flutter_engine_mut().platform_method_channel;
        platform_method_channel.invoke_method(
            "app_id_changed",
            Some(Box::new(json!({
                "surfaceId": surface_id,
                "appId": app_id,
            }))),
            None,
        );
    }

    fn title_changed(&mut self, surface: ToplevelSurface) {
        let surface_id = get_surface_id(surface.wl_surface());

        let title = with_states(surface.wl_surface(), |surface_data| {
            surface_data
                .data_map
                .get::<XdgToplevelSurfaceData>()
                .unwrap()
                .lock()
                .unwrap()
                .title
                .clone()
        });

        let platform_method_channel = &mut self.flutter_engine_mut().platform_method_channel;
        platform_method_channel.invoke_method(
            "title_changed",
            Some(Box::new(json!({
                "surfaceId": surface_id,
                "title": title,
            }))),
            None,
        );
    }
}
