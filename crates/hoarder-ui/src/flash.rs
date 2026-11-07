//! The optional camera flash (PLAN.md §4.2): a white layer surface on every
//! output that fades out. It only ever runs after the freeze, so it can never
//! end up in a Capture.

use std::os::fd::AsFd;
use std::os::unix::fs::FileExt;
use std::time::Duration;

use smithay_client_toolkit::{
    compositor::CompositorHandler,
    output::{OutputHandler, OutputState},
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    shell::{
        WaylandSurface,
        wlr_layer::{
            Anchor, KeyboardInteractivity, Layer, LayerShellHandler, LayerSurface,
            LayerSurfaceConfigure,
        },
    },
    shm::{Shm, ShmHandler},
};
use wayland_client::{
    Connection, QueueHandle, delegate_noop,
    protocol::{
        wl_buffer::WlBuffer,
        wl_output::{Transform, WlOutput},
        wl_region::WlRegion,
        wl_shm,
        wl_shm_pool::WlShmPool,
        wl_surface::WlSurface,
    },
};
use wayland_protocols::wp::viewporter::client::{
    wp_viewport::WpViewport, wp_viewporter::WpViewporter,
};

use crate::surface::{self, Error, Layered, failed};

/// Full white is held this long, then fades out over [`FADE`].
const HOLD: Duration = Duration::from_millis(60);
const FADE: Duration = Duration::from_millis(180);
/// Opacity steps of the fade, full white included.
const STEPS: usize = 8;

/// Flash every output white and return once it has faded. Best effort: the
/// caller only reports an error.
pub fn flash() -> Result<(), Error> {
    let Layered {
        conn,
        globals,
        mut queue,
        qh,
        compositor,
        layer_shell,
        shm,
    } = surface::connect::<Flash>("flash")?;
    let viewporter: WpViewporter = globals
        .bind(&qh, 1..=1, ())
        .map_err(|e| surface::missing("flash", "wp_viewporter", e))?;
    let mut app = Flash {
        registry: RegistryState::new(&globals),
        outputs: OutputState::new(&globals, &qh),
        shm,
        layers: Vec::new(),
    };
    // Learn the outputs.
    queue.roundtrip(&mut app).map_err(failed)?;

    // One 1×1 premultiplied ARGB pixel per step, from opaque white to clear.
    let fd = rustix::fs::memfd_create("hoarder-flash", rustix::fs::MemfdFlags::CLOEXEC)
        .map_err(failed)?;
    let file = std::fs::File::from(fd);
    let pixels: Vec<u8> = (0..STEPS)
        .flat_map(|i| {
            let a = (255 * (STEPS - 1 - i) / (STEPS - 1)) as u8;
            [a, a, a, a]
        })
        .collect();
    file.write_all_at(&pixels, 0).map_err(failed)?;
    let pool = app
        .shm
        .wl_shm()
        .create_pool(file.as_fd(), pixels.len() as i32, &qh, ());
    let buffers: Vec<WlBuffer> = (0..STEPS)
        .map(|i| pool.create_buffer(i as i32 * 4, 1, 1, 4, wl_shm::Format::Argb8888, &qh, ()))
        .collect();
    pool.destroy();

    let empty = compositor.wl_compositor().create_region(&qh, ());
    let outputs: Vec<WlOutput> = app.outputs.outputs().collect();
    for output in outputs {
        let surface = compositor.create_surface(&qh);
        let layer = layer_shell.create_layer_surface(
            &qh,
            surface,
            Layer::Overlay,
            Some("hoarder-flash"),
            Some(&output),
        );
        layer.set_anchor(Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT);
        layer.set_exclusive_zone(-1);
        layer.set_keyboard_interactivity(KeyboardInteractivity::None);
        // Clicks go straight through to whatever is underneath.
        layer.set_input_region(Some(&empty));
        layer.commit();
        let viewport = viewporter.get_viewport(layer.wl_surface(), &qh, ());
        app.layers.push(FlashLayer {
            layer,
            viewport,
            size: None,
        });
    }
    empty.destroy();
    queue.roundtrip(&mut app).map_err(failed)?;

    let show = |step: usize, app: &Flash| {
        for l in &app.layers {
            let Some((w, h)) = l.size else { continue };
            let surface = l.layer.wl_surface();
            l.viewport.set_destination(w as i32, h as i32);
            surface.attach(Some(&buffers[step]), 0, 0);
            surface.damage_buffer(0, 0, 1, 1);
            surface.commit();
        }
    };
    show(0, &app);
    queue.roundtrip(&mut app).map_err(failed)?;
    std::thread::sleep(HOLD);
    for step in 1..STEPS {
        show(step, &app);
        conn.flush().map_err(failed)?;
        std::thread::sleep(FADE / (STEPS - 1) as u32);
    }

    app.layers.clear();
    for buffer in buffers {
        buffer.destroy();
    }
    queue.roundtrip(&mut app).map_err(failed)?;
    Ok(())
}

struct FlashLayer {
    layer: LayerSurface,
    viewport: WpViewport,
    /// The size the compositor configured, once it has.
    size: Option<(u32, u32)>,
}

impl Drop for FlashLayer {
    fn drop(&mut self) {
        self.viewport.destroy();
    }
}

struct Flash {
    registry: RegistryState,
    outputs: OutputState,
    shm: Shm,
    layers: Vec<FlashLayer>,
}

impl LayerShellHandler for Flash {
    fn closed(&mut self, _: &Connection, _: &QueueHandle<Self>, layer: &LayerSurface) {
        if let Some(l) = self.layers.iter_mut().find(|l| &l.layer == layer) {
            l.size = None;
        }
    }

    fn configure(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        layer: &LayerSurface,
        configure: LayerSurfaceConfigure,
        _: u32,
    ) {
        let (w, h) = configure.new_size;
        if let Some(l) = self.layers.iter_mut().find(|l| &l.layer == layer) {
            l.size = (w > 0 && h > 0).then_some((w, h));
        }
    }
}

impl CompositorHandler for Flash {
    fn scale_factor_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlSurface,
        _: i32,
    ) {
    }

    fn transform_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlSurface,
        _: Transform,
    ) {
    }

    fn frame(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlSurface, _: u32) {}

    fn surface_enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlSurface,
        _: &WlOutput,
    ) {
    }

    fn surface_leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlSurface,
        _: &WlOutput,
    ) {
    }
}

impl OutputHandler for Flash {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.outputs
    }

    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: WlOutput) {}

    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: WlOutput) {}

    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: WlOutput) {}
}

impl ShmHandler for Flash {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

impl ProvidesRegistryState for Flash {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry
    }
    registry_handlers![OutputState];
}

smithay_client_toolkit::delegate_registry!(Flash);
smithay_client_toolkit::delegate_dispatch2!(Flash);
delegate_noop!(Flash: WlRegion);
delegate_noop!(Flash: WlShmPool);
delegate_noop!(Flash: ignore WlBuffer);
delegate_noop!(Flash: WpViewporter);
delegate_noop!(Flash: WpViewport);
