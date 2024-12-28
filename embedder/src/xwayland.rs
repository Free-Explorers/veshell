use crate::backend::Backend;
use crate::cursor::{load_cursor_theme, Cursor};
use crate::flutter_engine::wayland_messages::{MapX11Surface, NewX11Surface};
use crate::focus::KeyboardFocusTarget;
use crate::state::State;
use crate::wayland::wayland::get_surface_id;
use serde_json::json;

use smithay::input::pointer::CursorIcon;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Point, Rectangle, Size};
use smithay::wayland::seat::WaylandFocus;
use smithay::wayland::selection::data_device::{
    clear_data_device_selection, current_data_device_selection_userdata,
    request_data_device_client_selection, set_data_device_selection,
};
use smithay::wayland::selection::primary_selection::{
    clear_primary_selection, current_primary_selection_userdata, request_primary_client_selection,
    set_primary_selection,
};
use smithay::wayland::selection::SelectionTarget;
use smithay::wayland::xwayland_shell::{XWaylandShellHandler, XWaylandShellState};
use smithay::xwayland::xwm::{Reorder, WmWindowProperty, XwmId};
use smithay::xwayland::{xwm, X11Surface, X11Wm, XWayland, XWaylandEvent, XwmHandler};
use std::borrow::Borrow;
use std::cell::RefCell;
use std::os::fd::OwnedFd;
use tracing::{error, trace};

pub struct MyX11SurfaceState {
    pub x11_surface_id: u64,
}

impl<BackendData: Backend> State<BackendData> {
    pub fn start_xwayland(data: &mut State<BackendData>) {
        use std::process::Stdio;

        //XWaylandKeyboardGrabState::new::<Self>(&self.display_handle.clone());

        let (xwayland, client) = XWayland::spawn(
            &data.display_handle,
            None,
            std::iter::empty::<(String, String)>(),
            true,
            Stdio::null(),
            Stdio::null(),
            |_| (),
        )
        .expect("failed to start XWayland");

        let ret = data
            .loop_handle
            .insert_source(xwayland, move |event, _, data| match event {
                XWaylandEvent::Ready {
                    x11_socket,
                    display_number,
                } => {
                    let mut wm =
                        X11Wm::start_wm(data.loop_handle.clone(), x11_socket, client.clone())
                            .expect("Failed to attach X11 Window Manager");

                    let (theme, size) = load_cursor_theme();
                    let cursor = Cursor::load(&theme, CursorIcon::Default, size);
                    let image = cursor.get_image(1, 0);
                    wm.set_cursor(
                        &image.pixels_rgba,
                        Size::from((image.width as u16, image.height as u16)),
                        Point::from((image.xhot as u16, image.yhot as u16)),
                    )
                    .expect("Failed to set xwayland default cursor");

                    data.x11_wm = Some(wm);
                    data.xwayland_display = Some(display_number);

                    if let Some(flutter_engine) = data.flutter_engine.as_mut() {
                        flutter_engine.set_environment_variable(
                            "DISPLAY",
                            Some(format!(":{}", display_number).as_str()),
                        );
                    }
                }
                XWaylandEvent::Error => {
                    data.x11_wm = None;
                    data.xwayland_display = None;

                    if let Some(flutter_engine) = data.flutter_engine.as_mut() {
                        flutter_engine.set_environment_variable("DISPLAY", None);
                    }
                }
            });
        if let Err(e) = ret {
            tracing::error!(
                "Failed to insert the XWaylandSource into the event loop: {}",
                e
            );
        }
    }

    pub fn get_x11_surface_id(x11_surface: &X11Surface) -> u64 {
        x11_surface
            .user_data()
            .get::<RefCell<MyX11SurfaceState>>()
            .unwrap()
            .borrow()
            .x11_surface_id
    }

    fn new_x11_surface(&mut self, surface: X11Surface) {
        tracing::info!("new_x11_surface {:?}", surface.window_id());
        self.x11_surface_per_x11_window
            .insert(surface.window_id(), surface.clone());

        surface.user_data().insert_if_missing(|| {
            RefCell::new(MyX11SurfaceState {
                x11_surface_id: self.get_new_x11_surface_id(),
            })
        });

        let platform_method_channel = &mut self.flutter_engine_mut().platform_method_channel;
        platform_method_channel.invoke_method(
            "new_x11_surface",
            Some(Box::new(json!(NewX11Surface {
                x11_surface_id: Self::get_x11_surface_id(&surface),
                override_redirect: surface.is_override_redirect(),
            }))),
            None,
        );
    }

    pub fn map_x11_surface(&mut self, surface: X11Surface) {
        let parent = if surface.is_override_redirect() {
            surface
                .is_transient_for()
                .and_then(|x11_surface| self.x11_surface_per_x11_window.get(&x11_surface))
                // Fall back on the focused surface if the transient parent is not known.
                .or_else(|| {
                    self.keyboard.current_focus().and_then(|focus| {
                        focus
                            .wl_surface()
                            .and_then(|focus| self.x11_surface_per_wl_surface.get(focus.as_ref()))
                    })
                })
                .map(|x11_surface| Self::get_x11_surface_id(x11_surface))
        } else {
            surface
                .is_transient_for()
                .and_then(|x11_surface| self.x11_surface_per_x11_window.get(&x11_surface))
                .map(|x11_surface| Self::get_x11_surface_id(x11_surface))
        };

        let platform_method_channel = &mut self.flutter_engine_mut().platform_method_channel;
        platform_method_channel.invoke_method(
            "map_x11_surface",
            Some(Box::new(json!(MapX11Surface {
                x11_surface_id: Self::get_x11_surface_id(&surface),
                geometry: surface.geometry().into(),
                parent,
            }))),
            None,
        );
    }
}

impl<BackendData: Backend> XwmHandler for State<BackendData> {
    fn xwm_state(&mut self, _xwm: XwmId) -> &mut X11Wm {
        self.x11_wm.as_mut().unwrap()
    }

    fn new_window(&mut self, _xwm: XwmId, surface: X11Surface) {
        let mut geometry = surface.geometry();
        geometry.loc = Point::from((0, 0));
        surface.configure(geometry).unwrap();

        self.new_x11_surface(surface);
    }

    fn new_override_redirect_window(&mut self, _xwm: XwmId, surface: X11Surface) {
        self.new_x11_surface(surface);
    }

    fn map_window_request(&mut self, _xwm: XwmId, surface: X11Surface) {
        surface.set_mapped(true).unwrap();
        surface.set_activated(true).unwrap();
    }

    fn map_window_notify(&mut self, _xwm: XwmId, surface: X11Surface) {
        self.map_x11_surface(surface);
    }

    fn mapped_override_redirect_window(&mut self, _xwm: XwmId, surface: X11Surface) {
        self.map_x11_surface(surface.clone());
        surface.set_activated(true).unwrap();
    }

    fn unmapped_window(&mut self, _: XwmId, surface: X11Surface) {
        let Some(wl_surface) = surface.wl_surface() else {
            return;
        };

        self.x11_surface_per_wl_surface.remove(&wl_surface);

        let x11_surface_id = Self::get_x11_surface_id(&surface);

        let platform_method_channel = &mut self.flutter_engine_mut().platform_method_channel;
        platform_method_channel.invoke_method(
            "unmap_x11_surface",
            Some(Box::new(json!({
                "x11SurfaceId": x11_surface_id,
            }))),
            None,
        );

        if !surface.is_override_redirect() {
            surface.set_mapped(false).unwrap();
        }
    }

    fn destroyed_window(&mut self, _xwm: XwmId, surface: X11Surface) {
        let x11_surface_id = Self::get_x11_surface_id(&surface);

        let platform_method_channel = &mut self.flutter_engine_mut().platform_method_channel;
        platform_method_channel.invoke_method(
            "destroy_x11_surface",
            Some(Box::new(json!({
                "x11SurfaceId": x11_surface_id,
            }))),
            None,
        );

        self.x11_surface_per_x11_window.remove(&surface.window_id());
    }

    fn configure_request(
        &mut self,
        _xwm: XwmId,
        window: X11Surface,
        _x: Option<i32>,
        _y: Option<i32>,
        w: Option<u32>,
        h: Option<u32>,
        _reorder: Option<Reorder>,
    ) {
        // We just set the new size, but don't let windows move themselves around freely.
        let mut geo = window.geometry();
        geo.loc = Point::from((0, 0));
        if let Some(w) = w {
            geo.size.w = w as i32;
        }
        if let Some(h) = h {
            geo.size.h = h as i32;
        }
        let _ = window.configure(geo);
    }

    fn configure_notify(
        &mut self,
        _xwm: XwmId,
        _window: X11Surface,
        _geometry: Rectangle<i32, Logical>,
        _above: Option<u32>,
    ) {
    }

    fn property_notify(
        &mut self,
        _xwm: XwmId,
        x11_surface: X11Surface,
        _property: WmWindowProperty,
    ) {
        let platform_method_channel = &mut self.flutter_engine_mut().platform_method_channel;

        // to simplify we sent all properties to flutter when any changes
        // TODO: split each property to channel

        platform_method_channel.invoke_method(
            "x11_properties_changed",
            Some(Box::new(json!({
                "x11SurfaceId": Self::get_x11_surface_id(&x11_surface),
                "title": if !x11_surface.title().is_empty() { Some(x11_surface.title()) } else { None },
                "windowClass": x11_surface.class(),
                "instance": if !x11_surface.instance().is_empty() {
                    Some(x11_surface.instance())
                } else {
                    None
                },
                "startupId": x11_surface.startup_id(),
                "pid": x11_surface.pid(),
            }))),
            None,
        );

        /* match property {
            WmWindowProperty::Title => {
                let title = window.title();
                let title = if !title.is_empty() { Some(title) } else { None };

                platform_method_channel.invoke_method(
                    "title_changed",
                    Some(Box::new(json!({
                        "surfaceId": surface_id,
                        "title": title,
                    }))),
                    None,
                );
            }
            WmWindowProperty::Class => {
                let instance = window.instance();
                let instance = if !instance.is_empty() {
                    Some(instance)
                } else {
                    None
                };

                platform_method_channel.invoke_method(
                    "app_id_changed",
                    Some(Box::new(json!({
                        "surfaceId": surface_id,
                        "appId": instance,
                    }))),
                    None,
                );
            }
            _ => {}
        } */
    }

    fn resize_request(
        &mut self,
        _xwm: XwmId,
        _window: X11Surface,
        _button: u32,
        _resize_edge: xwm::ResizeEdge,
    ) {
        print!("resize_request");
    }

    fn move_request(&mut self, _xwm: XwmId, _window: X11Surface, _button: u32) {
        print!("move_request");
    }

    fn allow_selection_access(&mut self, xwm: XwmId, _selection: SelectionTarget) -> bool {
        if let Some(keyboard) = self.seat.get_keyboard() {
            // check that an X11 window is focused
            if let Some(KeyboardFocusTarget::X11Surface(surface)) = keyboard.current_focus() {
                if surface.xwm_id().unwrap() == xwm {
                    return true;
                }
            }
        }
        false
    }

    fn send_selection(
        &mut self,
        _xwm: XwmId,
        selection: SelectionTarget,
        mime_type: String,
        fd: OwnedFd,
    ) {
        match selection {
            SelectionTarget::Clipboard => {
                if let Err(err) = request_data_device_client_selection(&self.seat, mime_type, fd) {
                    error!(
                        ?err,
                        "Failed to request current wayland clipboard for Xwayland",
                    );
                }
            }
            SelectionTarget::Primary => {
                if let Err(err) = request_primary_client_selection(&self.seat, mime_type, fd) {
                    error!(
                        ?err,
                        "Failed to request current wayland primary selection for Xwayland",
                    );
                }
            }
        }
    }

    fn new_selection(&mut self, _xwm: XwmId, selection: SelectionTarget, mime_types: Vec<String>) {
        trace!(?selection, ?mime_types, "Got Selection from X11",);
        // TODO check, that focused windows is X11 window before doing this
        match selection {
            SelectionTarget::Clipboard => {
                set_data_device_selection(&self.display_handle, &self.seat, mime_types, ())
            }
            SelectionTarget::Primary => {
                set_primary_selection(&self.display_handle, &self.seat, mime_types, ())
            }
        }
    }

    fn cleared_selection(&mut self, _xwm: XwmId, selection: SelectionTarget) {
        match selection {
            SelectionTarget::Clipboard => {
                if current_data_device_selection_userdata(&self.seat).is_some() {
                    clear_data_device_selection(&self.display_handle, &self.seat)
                }
            }
            SelectionTarget::Primary => {
                if current_primary_selection_userdata(&self.seat).is_some() {
                    clear_primary_selection(&self.display_handle, &self.seat)
                }
            }
        }
    }
}

impl<BackendData: Backend> XWaylandShellHandler for State<BackendData> {
    fn xwayland_shell_state(&mut self) -> &mut XWaylandShellState {
        &mut self.xwayland_shell_state
    }

    fn surface_associated(&mut self, _xwm: XwmId, wl_surface: WlSurface, surface: X11Surface) {
        let x11_surface_id = Self::get_x11_surface_id(&surface);
        self.x11_surface_per_wl_surface
            .insert(wl_surface.clone(), surface.clone());
        let platform_method_channel = &mut self.flutter_engine_mut().platform_method_channel;

        platform_method_channel.invoke_method(
            "surface_associated",
            Some(Box::new(json!({
                "surfaceId": get_surface_id(wl_surface.borrow()),
                "x11SurfaceId": x11_surface_id,
            }))),
            None,
        );
    }
}
