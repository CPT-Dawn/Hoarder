//! The optional camera flash (PLAN.md §4.2): a white overlay on every output
//! that fades out. It only ever runs after the freeze, so it can never end up
//! in a Capture.

use std::os::fd::AsFd;
use std::os::unix::fs::FileExt;
use std::time::Duration;

use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::{
    wl_buffer::WlBuffer, wl_compositor::WlCompositor, wl_output::WlOutput, wl_region::WlRegion,
    wl_registry::WlRegistry, wl_shm, wl_shm::WlShm, wl_shm_pool::WlShmPool, wl_surface::WlSurface,
};
use wayland_client::{Connection, Dispatch, QueueHandle, delegate_noop};
use wayland_protocols::wp::viewporter::client::{
    wp_viewport::WpViewport, wp_viewporter::WpViewporter,
};
use wayland_protocols_wlr::layer_shell::v1::client::{
    zwlr_layer_shell_v1::{Layer, ZwlrLayerShellV1},
    zwlr_layer_surface_v1::{self, Anchor, KeyboardInteractivity, ZwlrLayerSurfaceV1},
};

/// Full white is held this long, then fades out over [`FADE`].
const HOLD: Duration = Duration::from_millis(60);
const FADE: Duration = Duration::from_millis(180);
/// Opacity steps of the fade, full white included.
const STEPS: usize = 8;

/// Flash every output white and return once it has faded. Best effort: any
/// missing protocol or error just means no flash.
pub fn flash() -> Result<(), String> {
    let conn = Connection::connect_to_env().map_err(|e| e.to_string())?;
    let (globals, mut queue) = registry_queue_init::<State>(&conn).map_err(|e| e.to_string())?;
    let qh = queue.handle();
    let bind_err = |e: wayland_client::globals::BindError| e.to_string();
    let compositor: WlCompositor = globals.bind(&qh, 1..=4, ()).map_err(bind_err)?;
    let shm: WlShm = globals.bind(&qh, 1..=1, ()).map_err(bind_err)?;
    let layer_shell: ZwlrLayerShellV1 = globals.bind(&qh, 1..=4, ()).map_err(bind_err)?;
    let viewporter: WpViewporter = globals.bind(&qh, 1..=1, ()).map_err(bind_err)?;
    let outputs: Vec<WlOutput> = globals.contents().with_list(|list| {
        list.iter()
            .filter(|g| g.interface == "wl_output")
            .map(|g| {
                globals
                    .registry()
                    .bind::<WlOutput, _, _>(g.name, g.version.min(4), &qh, ())
            })
            .collect()
    });

    // One 1×1 premultiplied ARGB pixel per step, from opaque white to clear.
    let fd = rustix::fs::memfd_create("hoarder-flash", rustix::fs::MemfdFlags::CLOEXEC)
        .map_err(|e| e.to_string())?;
    let file = std::fs::File::from(fd);
    let pixels: Vec<u8> = (0..STEPS)
        .flat_map(|i| {
            let a = (255 * (STEPS - 1 - i) / (STEPS - 1)) as u8;
            [a, a, a, a]
        })
        .collect();
    file.write_all_at(&pixels, 0).map_err(|e| e.to_string())?;
    let pool = shm.create_pool(file.as_fd(), pixels.len() as i32, &qh, ());
    let buffers: Vec<WlBuffer> = (0..STEPS)
        .map(|i| pool.create_buffer(i as i32 * 4, 1, 1, 4, wl_shm::Format::Argb8888, &qh, ()))
        .collect();
    pool.destroy();

    let mut state = State::default();
    let empty = compositor.create_region(&qh, ());
    let surfaces: Vec<_> = outputs
        .iter()
        .enumerate()
        .map(|(i, output)| {
            let surface = compositor.create_surface(&qh, ());
            // Clicks go straight through to whatever is underneath.
            surface.set_input_region(Some(&empty));
            let layer = layer_shell.get_layer_surface(
                &surface,
                Some(output),
                Layer::Overlay,
                "hoarder-flash".into(),
                &qh,
                i,
            );
            layer.set_anchor(Anchor::Top | Anchor::Bottom | Anchor::Left | Anchor::Right);
            layer.set_exclusive_zone(-1);
            layer.set_keyboard_interactivity(KeyboardInteractivity::None);
            surface.commit();
            state.sizes.push(None);
            let viewport = viewporter.get_viewport(&surface, &qh, ());
            (surface, layer, viewport)
        })
        .collect();
    queue.roundtrip(&mut state).map_err(|e| e.to_string())?;

    let show = |step: usize, state: &State| {
        for (i, (surface, _, viewport)) in surfaces.iter().enumerate() {
            let Some((w, h)) = state.sizes[i] else {
                continue;
            };
            viewport.set_destination(w as i32, h as i32);
            surface.attach(Some(&buffers[step]), 0, 0);
            surface.damage_buffer(0, 0, 1, 1);
            surface.commit();
        }
    };
    show(0, &state);
    queue.roundtrip(&mut state).map_err(|e| e.to_string())?;
    std::thread::sleep(HOLD);
    for step in 1..STEPS {
        show(step, &state);
        conn.flush().map_err(|e| e.to_string())?;
        std::thread::sleep(FADE / (STEPS - 1) as u32);
    }

    for (surface, layer, viewport) in surfaces {
        viewport.destroy();
        layer.destroy();
        surface.destroy();
    }
    for buffer in buffers {
        buffer.destroy();
    }
    queue.roundtrip(&mut state).map_err(|e| e.to_string())?;
    Ok(())
}

#[derive(Debug, Default)]
struct State {
    /// The size each layer surface was configured to, by output index.
    sizes: Vec<Option<(u32, u32)>>,
}

impl Dispatch<WlRegistry, GlobalListContents> for State {
    fn event(
        _: &mut Self,
        _: &WlRegistry,
        _: wayland_client::protocol::wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZwlrLayerSurfaceV1, usize> for State {
    fn event(
        state: &mut Self,
        layer: &ZwlrLayerSurfaceV1,
        event: zwlr_layer_surface_v1::Event,
        &i: &usize,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            zwlr_layer_surface_v1::Event::Configure {
                serial,
                width,
                height,
            } => {
                layer.ack_configure(serial);
                state.sizes[i] = (width > 0 && height > 0).then_some((width, height));
            }
            zwlr_layer_surface_v1::Event::Closed => state.sizes[i] = None,
            _ => {}
        }
    }
}

delegate_noop!(State: ignore WlCompositor);
delegate_noop!(State: ignore WlShm);
delegate_noop!(State: ignore WlShmPool);
delegate_noop!(State: ignore WlBuffer);
delegate_noop!(State: ignore WlSurface);
delegate_noop!(State: ignore WlRegion);
delegate_noop!(State: ignore WlOutput);
delegate_noop!(State: ignore ZwlrLayerShellV1);
delegate_noop!(State: ignore WpViewporter);
delegate_noop!(State: ignore WpViewport);
