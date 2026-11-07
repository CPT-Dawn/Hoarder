//! Toplevel capture (PLAN.md §4.1, Window Targets): the windows the
//! compositor lists over `ext-foreign-toplevel-list-v1`, and a copy of one
//! window's own buffer through the ext foreign-toplevel image capture
//! source, whole even where other windows cover it.
//!
//! A toplevel is named by its `identifier`, which the compositor never
//! reuses and which is the same for every client, so [`list`] and
//! [`capture`] each use their own connection. On Sway the identifier is the
//! `foreign_toplevel_identifier` of `get_tree`; on Hyprland,
//! `hyprland_toplevel_mapping_v1` gives each toplevel's window address.

use std::collections::HashSet;
use std::os::fd::AsFd;
use std::os::unix::fs::FileExt;

use wayland_client::globals::{GlobalList, GlobalListContents, registry_queue_init};
use wayland_client::protocol::{
    wl_buffer::WlBuffer, wl_registry::WlRegistry, wl_shm::WlShm, wl_shm_pool::WlShmPool,
};
use wayland_client::{
    Connection, Dispatch, EventQueue, Proxy, QueueHandle, WEnum, delegate_noop, event_created_child,
};
use wayland_protocols::ext::foreign_toplevel_list::v1::client::{
    ext_foreign_toplevel_handle_v1::{self, ExtForeignToplevelHandleV1},
    ext_foreign_toplevel_list_v1::{self, ExtForeignToplevelListV1},
};
use wayland_protocols::ext::image_capture_source::v1::client::{
    ext_foreign_toplevel_image_capture_source_manager_v1::ExtForeignToplevelImageCaptureSourceManagerV1,
    ext_image_capture_source_v1::ExtImageCaptureSourceV1,
};
use wayland_protocols::ext::image_copy_capture::v1::client::{
    ext_image_copy_capture_frame_v1::{self, ExtImageCopyCaptureFrameV1},
    ext_image_copy_capture_manager_v1::{ExtImageCopyCaptureManagerV1, Options},
    ext_image_copy_capture_session_v1::{self, ExtImageCopyCaptureSessionV1},
};

use crate::capture::HIDE_GLOBALS_ENV;
use crate::hyprland::toplevel_mapping::{
    hyprland_toplevel_mapping_manager_v1::HyprlandToplevelMappingManagerV1,
    hyprland_toplevel_window_mapping_handle_v1::{self, HyprlandToplevelWindowMappingHandleV1},
};
use crate::{CaptureError, ShmFormat, Transform};

const LIST: &str = "ext_foreign_toplevel_list_v1";
const SOURCE: &str = "ext_foreign_toplevel_image_capture_source_manager_v1";
const COPY: &str = "ext_image_copy_capture_manager_v1";
const MAPPING: &str = "hyprland_toplevel_mapping_manager_v1";

/// A window as `ext-foreign-toplevel-list-v1` lists it: every one the
/// compositor manages, on any workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Toplevel {
    /// Unique for the compositor's lifetime, never reused.
    pub identifier: String,
    pub title: String,
    pub app_id: String,
    /// Its window address, from `hyprland_toplevel_mapping_v1` on Hyprland.
    pub hyprland_address: Option<u64>,
}

/// One window's buffer exactly as the compositor wrote it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToplevelFrame {
    pub format: ShmFormat,
    /// Buffer size in pixels, before the transform is undone.
    pub width: u32,
    pub height: u32,
    /// Bytes per row.
    pub stride: u32,
    pub transform: Transform,
    pub data: Vec<u8>,
}

/// Every window the compositor lists, in its order.
pub fn list() -> Result<Vec<Toplevel>, CaptureError> {
    let lister = Lister::connect()?;
    Ok(lister.state.toplevels.into_iter().map(|t| t.info).collect())
}

/// Copy the buffer of the window named `identifier`.
pub fn capture(identifier: &str) -> Result<ToplevelFrame, CaptureError> {
    let mut lister = Lister::connect()?;
    let handle = lister
        .state
        .toplevels
        .iter()
        .find(|t| t.info.identifier == identifier)
        .map(|t| t.handle.clone())
        .ok_or_else(|| CaptureError::Failed("the window has gone away".into()))?;
    lister.capture(&handle)
}

struct Lister {
    globals: GlobalList,
    hidden: HashSet<String>,
    queue: EventQueue<State>,
    state: State,
}

impl Lister {
    /// Connect and read the whole window list.
    fn connect() -> Result<Lister, CaptureError> {
        let conn =
            Connection::connect_to_env().map_err(|e| CaptureError::NoWayland(e.to_string()))?;
        let (globals, queue) = registry_queue_init::<State>(&conn)
            .map_err(|e| CaptureError::NoWayland(e.to_string()))?;
        let hidden = std::env::var(HIDE_GLOBALS_ENV)
            .unwrap_or_default()
            .split(',')
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty())
            .collect();
        let mut lister = Lister {
            globals,
            hidden,
            queue,
            state: State::default(),
        };
        let list: ExtForeignToplevelListV1 = lister.bind(LIST)?;
        // The list arrives in one burst after the bind; each handle's details
        // follow it, up to its `done`.
        lister.roundtrip()?;
        lister.roundtrip()?;
        list.stop();
        lister.state.toplevels.retain(|t| t.done && !t.closed);
        lister.map_to_hyprland()?;
        Ok(lister)
    }

    /// On Hyprland, ask each toplevel's window address.
    fn map_to_hyprland(&mut self) -> Result<(), CaptureError> {
        if !self.has(MAPPING) {
            return Ok(());
        }
        let manager: HyprlandToplevelMappingManagerV1 = self.bind(MAPPING)?;
        let qh = self.queue.handle();
        let handles: Vec<_> = self
            .state
            .toplevels
            .iter()
            .enumerate()
            .map(|(i, t)| manager.get_window_for_toplevel(&t.handle, &qh, i))
            .collect();
        self.roundtrip()?;
        for handle in handles {
            handle.destroy();
        }
        manager.destroy();
        Ok(())
    }

    fn has(&self, interface: &str) -> bool {
        !self.hidden.contains(interface)
            && self
                .globals
                .contents()
                .with_list(|list| list.iter().any(|g| g.interface == interface))
    }

    fn bind<P>(&self, interface: &str) -> Result<P, CaptureError>
    where
        P: Proxy + 'static,
        State: Dispatch<P, ()>,
    {
        if !self.has(interface) {
            return Err(CaptureError::Unsupported(format!(
                "the compositor does not offer {interface}, so windows cannot be captured on their own"
            )));
        }
        self.globals
            .bind(&self.queue.handle(), 1..=1, ())
            .map_err(|e| CaptureError::Unsupported(format!("cannot bind {interface}: {e}")))
    }

    fn roundtrip(&mut self) -> Result<(), CaptureError> {
        self.queue
            .roundtrip(&mut self.state)
            .map(|_| ())
            .map_err(|e| CaptureError::Failed(e.to_string()))
    }

    fn dispatch_until(
        &mut self,
        what: &str,
        done: impl Fn(&Copy) -> bool,
    ) -> Result<(), CaptureError> {
        while !done(&self.state.copy) {
            self.queue
                .blocking_dispatch(&mut self.state)
                .map_err(|e| CaptureError::Failed(format!("{what}: {e}")))?;
            if let Some(why) = self.state.copy.failure.take() {
                return Err(CaptureError::Failed(why));
            }
        }
        Ok(())
    }

    fn capture(
        &mut self,
        handle: &ExtForeignToplevelHandleV1,
    ) -> Result<ToplevelFrame, CaptureError> {
        let copy: ExtImageCopyCaptureManagerV1 = self.bind(COPY)?;
        let sources: ExtForeignToplevelImageCaptureSourceManagerV1 = self.bind(SOURCE)?;
        let qh = self.queue.handle();
        let source = sources.create_source(handle, &qh, ());
        let session = copy.create_session(&source, Options::empty(), &qh, ());
        source.destroy();
        self.dispatch_until("window capture constraints", |c| c.constraints_done)?;

        let format = ShmFormat::ALL
            .into_iter()
            .find(|f| self.state.copy.offered.contains(&(f.wl() as u32)))
            .ok_or_else(|| {
                CaptureError::Unsupported(format!(
                    "the compositor offers no readable shm format for the window (offered: {:x?})",
                    self.state.copy.offered
                ))
            })?;
        let (width, height) = self.state.copy.size;
        let stride = width * 4;
        let shm: WlShm = self
            .globals
            .bind(&qh, 1..=1, ())
            .map_err(|e| CaptureError::Unsupported(format!("cannot bind wl_shm: {e}")))?;
        let size = stride as usize * height as usize;
        let fd = rustix::fs::memfd_create("hoarder-window", rustix::fs::MemfdFlags::CLOEXEC)
            .map_err(std::io::Error::from)?;
        let file = std::fs::File::from(fd);
        file.set_len(size as u64)?;
        let pool = shm.create_pool(file.as_fd(), size as i32, &qh, ());
        let buffer = pool.create_buffer(
            0,
            width as i32,
            height as i32,
            stride as i32,
            format.wl(),
            &qh,
            (),
        );
        pool.destroy();

        let frame = session.create_frame(&qh, ());
        frame.attach_buffer(&buffer);
        frame.damage_buffer(0, 0, width as i32, height as i32);
        frame.capture();
        self.dispatch_until("window capture", |c| c.ready)?;
        frame.destroy();
        session.destroy();

        let mut data = vec![0u8; size];
        file.read_exact_at(&mut data, 0)?;
        buffer.destroy();
        Ok(ToplevelFrame {
            format,
            width,
            height,
            stride,
            transform: self.state.copy.transform,
            data,
        })
    }
}

struct Listed {
    handle: ExtForeignToplevelHandleV1,
    info: Toplevel,
    done: bool,
    closed: bool,
}

#[derive(Default)]
struct State {
    toplevels: Vec<Listed>,
    copy: Copy,
}

/// Progress of the one capture a connection makes.
struct Copy {
    size: (u32, u32),
    offered: Vec<u32>,
    constraints_done: bool,
    transform: Transform,
    ready: bool,
    failure: Option<String>,
}

impl Default for Copy {
    fn default() -> Self {
        Copy {
            size: (0, 0),
            offered: Vec::new(),
            constraints_done: false,
            transform: Transform::Normal,
            ready: false,
            failure: None,
        }
    }
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

impl Dispatch<ExtForeignToplevelListV1, ()> for State {
    fn event(
        state: &mut Self,
        _: &ExtForeignToplevelListV1,
        event: ext_foreign_toplevel_list_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let ext_foreign_toplevel_list_v1::Event::Toplevel { toplevel } = event {
            state.toplevels.push(Listed {
                handle: toplevel,
                info: Toplevel {
                    identifier: String::new(),
                    title: String::new(),
                    app_id: String::new(),
                    hyprland_address: None,
                },
                done: false,
                closed: false,
            });
        }
    }

    event_created_child!(State, ExtForeignToplevelListV1, [
        ext_foreign_toplevel_list_v1::EVT_TOPLEVEL_OPCODE => (ExtForeignToplevelHandleV1, ()),
    ]);
}

impl Dispatch<ExtForeignToplevelHandleV1, ()> for State {
    fn event(
        state: &mut Self,
        handle: &ExtForeignToplevelHandleV1,
        event: ext_foreign_toplevel_handle_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use ext_foreign_toplevel_handle_v1::Event as E;
        let Some(t) = state.toplevels.iter_mut().find(|t| &t.handle == handle) else {
            return;
        };
        match event {
            E::Identifier { identifier } => t.info.identifier = identifier,
            E::Title { title } => t.info.title = title,
            E::AppId { app_id } => t.info.app_id = app_id,
            E::Done => t.done = true,
            E::Closed => t.closed = true,
            _ => {}
        }
    }
}

impl Dispatch<ExtImageCopyCaptureSessionV1, ()> for State {
    fn event(
        state: &mut Self,
        _: &ExtImageCopyCaptureSessionV1,
        event: ext_image_copy_capture_session_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use ext_image_copy_capture_session_v1::Event as E;
        let copy = &mut state.copy;
        match event {
            E::BufferSize { width, height } => copy.size = (width, height),
            E::ShmFormat { format } => copy.offered.push(match format {
                WEnum::Value(f) => f as u32,
                WEnum::Unknown(code) => code,
            }),
            E::Done => copy.constraints_done = true,
            E::Stopped if !copy.ready => {
                copy.failure = Some("the window went away during the capture".into());
            }
            _ => {}
        }
    }
}

impl Dispatch<ExtImageCopyCaptureFrameV1, ()> for State {
    fn event(
        state: &mut Self,
        _: &ExtImageCopyCaptureFrameV1,
        event: ext_image_copy_capture_frame_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use ext_image_copy_capture_frame_v1::Event as E;
        match event {
            E::Transform { transform } => state.copy.transform = Transform::from_wl(transform),
            E::Ready => state.copy.ready = true,
            E::Failed { reason } => {
                state.copy.failure = Some(format!(
                    "the compositor could not copy the window ({reason:?})"
                ))
            }
            _ => {}
        }
    }
}

impl Dispatch<HyprlandToplevelWindowMappingHandleV1, usize> for State {
    fn event(
        state: &mut Self,
        _: &HyprlandToplevelWindowMappingHandleV1,
        event: hyprland_toplevel_window_mapping_handle_v1::Event,
        &i: &usize,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let hyprland_toplevel_window_mapping_handle_v1::Event::WindowAddress {
            address_hi,
            address,
        } = event
        {
            state.toplevels[i].info.hyprland_address =
                Some((address_hi as u64) << 32 | address as u64);
        }
    }
}

delegate_noop!(State: HyprlandToplevelMappingManagerV1);
delegate_noop!(State: ignore WlShm);
delegate_noop!(State: WlShmPool);
delegate_noop!(State: ignore WlBuffer);
delegate_noop!(State: ExtImageCopyCaptureManagerV1);
delegate_noop!(State: ExtForeignToplevelImageCaptureSourceManagerV1);
delegate_noop!(State: ExtImageCaptureSourceV1);
