pub mod xdg;

#[allow(clippy::module_inception)]
pub mod wayland {
    use std::cell::RefCell;

    use serde_json::json;
    use smithay::{
        backend::renderer::{
            gles::GlesRenderer,
            utils::{import_surface, on_commit_buffer_handler, RendererSurfaceStateUserData},
            Renderer, Texture,
        },
        input::pointer::{CursorImageStatus, CursorImageSurfaceData},
        reexports::wayland_server::{protocol::wl_surface::WlSurface, Client},
        utils::{Buffer as BufferCoords, Size},
        wayland::compositor::{
            with_states, with_surface_tree_upward, CompositorClientState, CompositorHandler,
            CompositorState, SurfaceAttributes, TraversalAction,
        },
        xwayland::XWaylandClientData,
    };
    use tracing::info;

    use crate::{state::State, Backend, ClientState};

    pub struct WlSurfaceVeshellState {
        pub surface_id: u64,
        pub old_texture_size: Option<Size<i32, BufferCoords>>,
    }

    pub fn get_surface_id(surface: &WlSurface) -> u64 {
        with_states(surface, |surface_data| {
            surface_data
                .data_map
                .get::<RefCell<WlSurfaceVeshellState>>()
                .unwrap()
                .borrow()
                .surface_id
        })
    }

    pub fn get_direct_subsurfaces(surface: &WlSurface) -> (Vec<u64>, Vec<u64>) {
        let mut subsurfaces_below = vec![];
        let mut subsurfaces_above = vec![];
        let mut above = false;

        with_surface_tree_upward(
            surface,
            (),
            |child_surface, _, ()| {
                // Only traverse the direct children of the surface
                if child_surface == surface {
                    TraversalAction::DoChildren(())
                } else {
                    TraversalAction::SkipChildren
                }
            },
            |child_surface, child_surface_data, ()| {
                if child_surface == surface {
                    above = true;
                    return;
                }

                let surface_id = child_surface_data
                    .data_map
                    .get::<RefCell<WlSurfaceVeshellState>>()
                    .unwrap()
                    .borrow()
                    .surface_id;
                if above {
                    subsurfaces_above.push(surface_id);
                } else {
                    subsurfaces_below.push(surface_id);
                }
            },
            |_, _, _| true,
        );
        (subsurfaces_below, subsurfaces_above)
    }

    impl<BackendData: Backend> CompositorHandler for State<BackendData> {
        fn compositor_state(&mut self) -> &mut CompositorState {
            &mut self.compositor_state
        }

        fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState {
            if let Some(state) = client.get_data::<XWaylandClientData>() {
                return &state.compositor_state;
            }
            if let Some(state) = client.get_data::<ClientState>() {
                return &state.compositor_state;
            }
            panic!("Unknown client data type")
        }

        fn new_surface(&mut self, surface: &WlSurface) {
            let surface_id = self.get_new_surface_id();
            with_states(surface, |surface_data| {
                surface_data.data_map.insert_if_missing(|| {
                    RefCell::new(WlSurfaceVeshellState {
                        surface_id,
                        old_texture_size: None,
                    })
                })
            });
            self.surfaces.insert(surface_id, surface.clone());

            let platform_method_channel = &mut self.flutter_engine_mut().platform_method_channel;
            platform_method_channel.invoke_method(
                "new_surface",
                Some(Box::new(json!({
                    "surfaceId": surface_id,
                }))),
                None,
            );
        }

        fn new_subsurface(&mut self, surface: &WlSurface, parent: &WlSurface) {
            let surface_id = get_surface_id(surface);
            let parent = get_surface_id(parent);
            self.subsurfaces.insert(surface_id, surface.clone());
            let platform_method_channel = &mut self.flutter_engine_mut().platform_method_channel;
            platform_method_channel.invoke_method(
                "new_subsurface",
                Some(Box::new(json!({
                    "surfaceId": surface_id,
                    "parent": parent,
                }))),
                None,
            );
        }

        fn commit(&mut self, surface: &WlSurface) {
            on_commit_buffer_handler::<Self>(surface);

            let is_cursor_image = matches!(*self.cursor_image_status.lock().unwrap(),CursorImageStatus::Surface(ref cursor_surface) if cursor_surface == surface);

            if is_cursor_image {
                info!("commit cursor image");
                with_states(surface, |states| {
                    let cursor_image_attributes = states.data_map.get::<CursorImageSurfaceData>();

                    if let Some(mut cursor_image_attributes) =
                        cursor_image_attributes.map(|attrs| attrs.lock().unwrap())
                    {
                        let buffer_delta = states
                            .cached_state
                            .get::<SurfaceAttributes>()
                            .current()
                            .buffer_delta
                            .take();
                        if let Some(buffer_delta) = buffer_delta {
                            cursor_image_attributes.hotspot -= buffer_delta;
                        }
                    }
                });
            }

            let (subsurfaces_below, subsurfaces_above) = get_direct_subsurfaces(surface);

            // Make sure Flutter knows about subsurfaces
            // because Wayland clients have the option to never commit them.
            // In Wayland, when the parent surface is committed,
            // subsurfaces are also committed recursively.
            for surface_id in subsurfaces_below.iter().chain(subsurfaces_above.iter()) {
                let surface = self.surfaces.get(surface_id).unwrap().clone();
                self.commit(&surface);
            }

            with_states(surface, |surface_data| {
                self.backend_data.with_primary_renderer_mut(|renderer| {
                    if let Err(err) = import_surface(renderer, surface_data) {
                        tracing::error!("Failed to import surface: {:?}", err);
                    }
                });
                let (surface_id, old_texture_size) = {
                    let my_state = surface_data
                        .data_map
                        .get::<RefCell<WlSurfaceVeshellState>>()
                        .unwrap()
                        .borrow();
                    (my_state.surface_id, my_state.old_texture_size)
                };

                let mut binding = surface_data.cached_state.get::<SurfaceAttributes>();
                let attributes = binding.current();

                let (texture_id, size) = if let Some(data) =
                    surface_data.data_map.get::<RendererSurfaceStateUserData>()
                {
                    let mut data_ref = data.lock().unwrap();
                    let renderer_state = &mut *data_ref;
                    let texture = self
                        .backend_data
                        .with_primary_renderer(|renderer| {
                            renderer_state.texture::<GlesRenderer>(renderer.id())
                        })
                        .unwrap();

                    if let Some(texture) = texture {
                        let size = texture.size();

                        let size_changed = match old_texture_size {
                            Some(old_size) => old_size != size,
                            None => true,
                        };

                        surface_data
                            .data_map
                            .get::<RefCell<WlSurfaceVeshellState>>()
                            .unwrap()
                            .borrow_mut()
                            .old_texture_size = Some(size);

                        let texture_id = match size_changed {
                            true => None,
                            false => self
                                .texture_ids_per_surface_id
                                .get(&surface_id)
                                .and_then(|v| v.last().cloned())
                                .map(|(id, _)| id),
                        };

                        let texture_id = texture_id.unwrap_or_else(|| {
                            let texture_id = self.get_new_texture_id();
                            while self
                                .texture_ids_per_surface_id
                                .entry(surface_id)
                                .or_default()
                                .len()
                                >= 2
                            {
                                self.texture_ids_per_surface_id
                                    .entry(surface_id)
                                    .or_default()
                                    .remove(0);
                            }

                            self.texture_ids_per_surface_id
                                .entry(surface_id)
                                .or_default()
                                .push((texture_id, size));
                            self.surface_id_per_texture_id
                                .insert(texture_id, surface_id);
                            self.flutter_engine_mut()
                                .register_external_texture(texture_id)
                                .unwrap();
                            texture_id
                        });

                        let swapchain = self.texture_swapchains.entry(texture_id).or_default();
                        swapchain.commit(texture.clone());

                        self.flutter_engine_mut()
                            .mark_external_texture_frame_available(texture_id)
                            .unwrap();

                        (texture_id, Some(size))
                    } else {
                        (-1, None)
                    }
                } else {
                    (-1, None)
                };

                (
                    surface_id,
                    texture_id,
                    size,
                    attributes.buffer_delta,
                    attributes.buffer_scale,
                    attributes.input_region.clone(),
                )
            });
            let surface_message = self.construct_surface_message(surface);

            let platform_method_channel = &mut self.flutter_engine_mut().platform_method_channel;
            platform_method_channel.invoke_method(
                "commit_surface",
                Some(Box::new(json!(surface_message))),
                None,
            );
        }

        fn destroyed(&mut self, _surface: &WlSurface) {
            let surface_id = with_states(_surface, |surface_data| {
                surface_data
                    .data_map
                    .get::<RefCell<WlSurfaceVeshellState>>()
                    .unwrap()
                    .borrow()
                    .surface_id
            });
            self.surfaces.remove(&surface_id);

            let platform_method_channel = &mut self.flutter_engine_mut().platform_method_channel;
            platform_method_channel.invoke_method(
                "destroy_surface",
                Some(Box::new(json!({
                    "surfaceId": surface_id,
                }))),
                None,
            );
        }
    }
}
