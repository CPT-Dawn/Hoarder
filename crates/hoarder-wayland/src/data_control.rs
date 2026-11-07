//! The two data-control protocols, `ext-data-control-v1` and its fallback
//! `wlr-data-control-unstable-v1`, are the same shape. These wrappers let
//! [`crate::clipboard`] and [`crate::watch`] ignore which one is in use.

use std::os::fd::BorrowedFd;

use wayland_client::backend::ObjectId;
use wayland_client::globals::GlobalList;
use wayland_client::protocol::wl_seat::WlSeat;
use wayland_client::{Dispatch, Proxy, QueueHandle};
use wayland_protocols::ext::data_control::v1::client::{
    ext_data_control_device_v1::ExtDataControlDeviceV1,
    ext_data_control_manager_v1::ExtDataControlManagerV1,
    ext_data_control_offer_v1::ExtDataControlOfferV1,
    ext_data_control_source_v1::ExtDataControlSourceV1,
};
use wayland_protocols_wlr::data_control::v1::client::{
    zwlr_data_control_device_v1::ZwlrDataControlDeviceV1,
    zwlr_data_control_manager_v1::ZwlrDataControlManagerV1,
    zwlr_data_control_offer_v1::ZwlrDataControlOfferV1,
    zwlr_data_control_source_v1::ZwlrDataControlSourceV1,
};

use crate::clipboard::{ClipboardError, Selection, hidden_globals};

const EXT_DATA_CONTROL: &str = "ext_data_control_manager_v1";
const WLR_DATA_CONTROL: &str = "zwlr_data_control_manager_v1";

/// Which data-control protocol a connection ended up with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataControl {
    /// `ext-data-control-v1`.
    Ext,
    /// `wlr-data-control-unstable-v1`, the fallback.
    Wlr,
}

impl DataControl {
    pub fn name(self) -> &'static str {
        match self {
            DataControl::Ext => "ext",
            DataControl::Wlr => "wlr",
        }
    }
}

#[derive(Debug)]
pub(crate) enum Manager {
    Ext(ExtDataControlManagerV1),
    Wlr(ZwlrDataControlManagerV1),
}

#[derive(Debug)]
pub(crate) enum Device {
    Ext(ExtDataControlDeviceV1),
    Wlr(ZwlrDataControlDeviceV1),
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum OfferProxy {
    Ext(ExtDataControlOfferV1),
    Wlr(ZwlrDataControlOfferV1),
}

#[derive(Debug)]
pub(crate) enum Source {
    Ext(ExtDataControlSourceV1),
    Wlr(ZwlrDataControlSourceV1),
}

impl Manager {
    /// Bind ext data control, or wlr's where ext is missing (or hidden by
    /// `HOARDER_HIDE_GLOBALS`). Neither is [`ClipboardError::Unsupported`],
    /// saying it is what `needed_for` needs.
    pub(crate) fn bind<D>(
        globals: &GlobalList,
        qh: &QueueHandle<D>,
        needed_for: &str,
    ) -> Result<Manager, ClipboardError>
    where
        D: Dispatch<ExtDataControlManagerV1, ()> + Dispatch<ZwlrDataControlManagerV1, ()> + 'static,
    {
        let hidden = hidden_globals();
        let available = |name: &str| {
            !hidden.contains(name)
                && globals
                    .contents()
                    .with_list(|list| list.iter().any(|g| g.interface == name))
        };
        let bind_error = |e| ClipboardError::Unsupported(format!("cannot bind data control: {e}"));
        if available(EXT_DATA_CONTROL) {
            Ok(Manager::Ext(
                globals.bind(qh, 1..=1, ()).map_err(bind_error)?,
            ))
        } else if available(WLR_DATA_CONTROL) {
            Ok(Manager::Wlr(
                globals.bind(qh, 1..=2, ()).map_err(bind_error)?,
            ))
        } else {
            Err(ClipboardError::Unsupported(format!(
                "this compositor offers neither ext-data-control-v1 nor \
                 wlr-data-control-unstable-v1, which {needed_for}"
            )))
        }
    }

    pub(crate) fn kind(&self) -> DataControl {
        match self {
            Manager::Ext(_) => DataControl::Ext,
            Manager::Wlr(_) => DataControl::Wlr,
        }
    }

    pub(crate) fn get_data_device<D>(&self, seat: &WlSeat, qh: &QueueHandle<D>) -> Device
    where
        D: Dispatch<ExtDataControlDeviceV1, ()> + Dispatch<ZwlrDataControlDeviceV1, ()> + 'static,
    {
        match self {
            Manager::Ext(m) => Device::Ext(m.get_data_device(seat, qh, ())),
            Manager::Wlr(m) => Device::Wlr(m.get_data_device(seat, qh, ())),
        }
    }

    pub(crate) fn create_data_source<D>(&self, qh: &QueueHandle<D>, which: Selection) -> Source
    where
        D: Dispatch<ExtDataControlSourceV1, Selection>
            + Dispatch<ZwlrDataControlSourceV1, Selection>
            + 'static,
    {
        match self {
            Manager::Ext(m) => Source::Ext(m.create_data_source(qh, which)),
            Manager::Wlr(m) => Source::Wlr(m.create_data_source(qh, which)),
        }
    }
}

impl Device {
    pub(crate) fn set_selection(&self, which: Selection, source: Option<&Source>) {
        match (self, which) {
            (Device::Ext(d), Selection::Clipboard) => d.set_selection(source.map(Source::ext)),
            (Device::Ext(d), Selection::Primary) => {
                d.set_primary_selection(source.map(Source::ext))
            }
            (Device::Wlr(d), Selection::Clipboard) => d.set_selection(source.map(Source::wlr)),
            (Device::Wlr(d), Selection::Primary) => {
                d.set_primary_selection(source.map(Source::wlr))
            }
        }
    }
}

impl OfferProxy {
    pub(crate) fn id(&self) -> ObjectId {
        match self {
            OfferProxy::Ext(o) => o.id(),
            OfferProxy::Wlr(o) => o.id(),
        }
    }

    pub(crate) fn receive(&self, mime_type: &str, fd: BorrowedFd<'_>) {
        match self {
            OfferProxy::Ext(o) => o.receive(mime_type.to_owned(), fd),
            OfferProxy::Wlr(o) => o.receive(mime_type.to_owned(), fd),
        }
    }

    pub(crate) fn destroy(&self) {
        match self {
            OfferProxy::Ext(o) => o.destroy(),
            OfferProxy::Wlr(o) => o.destroy(),
        }
    }
}

impl Source {
    pub(crate) fn id(&self) -> ObjectId {
        match self {
            Source::Ext(s) => s.id(),
            Source::Wlr(s) => s.id(),
        }
    }

    pub(crate) fn offer(&self, mime_type: &str) {
        match self {
            Source::Ext(s) => s.offer(mime_type.to_owned()),
            Source::Wlr(s) => s.offer(mime_type.to_owned()),
        }
    }

    pub(crate) fn destroy(&self) {
        match self {
            Source::Ext(s) => s.destroy(),
            Source::Wlr(s) => s.destroy(),
        }
    }

    fn ext(&self) -> &ExtDataControlSourceV1 {
        match self {
            Source::Ext(s) => s,
            Source::Wlr(_) => unreachable!("sources match the device's protocol"),
        }
    }

    fn wlr(&self) -> &ZwlrDataControlSourceV1 {
        match self {
            Source::Wlr(s) => s,
            Source::Ext(_) => unreachable!("sources match the device's protocol"),
        }
    }
}
