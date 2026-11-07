//! The History key in the Secret Service keyring (PLAN.md §5.3), through
//! oo7. Only the default collection is used, and Hoarder never prompts: a
//! locked or absent keyring is reported, and the next History request
//! tries again.

use hoarder_store::Key;
use oo7::dbus::{Error, Service, ServiceError};

/// How the History key's item is found again.
const ATTRIBUTES: [(&str, &str); 2] = [("application", "hoarder"), ("purpose", "history-key")];
const LABEL: &str = "Hoarder History encryption key";

const LOCKED: &str = "the keyring is locked; unlock it (log in again, or open Seahorse or \
                      KeePassXC) and History opens on its next use";

/// The History key from the keyring. With `create`, a missing key is made
/// and stored; otherwise it is `None`.
pub(super) fn history_key(create: bool) -> Result<Option<Key>, String> {
    zbus::block_on(fetch(create))
}

async fn fetch(create: bool) -> Result<Option<Key>, String> {
    let service = Service::new().await.map_err(explain)?;
    let collection = service
        .with_alias(Service::DEFAULT_COLLECTION)
        .await
        .map_err(explain)?
        .ok_or(
            "the Secret Service has no default keyring; create one (e.g. in Seahorse) so \
             Hoarder can keep the History key there",
        )?;
    if collection.is_locked().await.map_err(explain)? {
        return Err(LOCKED.into());
    }
    let items = collection
        .search_items(&ATTRIBUTES)
        .await
        .map_err(explain)?;
    if let Some(item) = items.first() {
        if item.is_locked().await.map_err(explain)? {
            return Err(LOCKED.into());
        }
        let secret = item.secret().await.map_err(explain)?;
        let key = std::str::from_utf8(secret.as_bytes())
            .ok()
            .and_then(|s| Key::parse(s).ok())
            .ok_or("the History key in the keyring is malformed")?;
        return Ok(Some(key));
    }
    if !create {
        return Ok(None);
    }
    let key = Key::generate();
    collection
        .create_item(LABEL, &ATTRIBUTES, key.secret().as_str(), true, None)
        .await
        .map_err(explain)?;
    Ok(Some(key))
}

/// Say what went wrong in terms of what the user can fix.
fn explain(e: Error) -> String {
    let wire = match &e {
        Error::ZBus(z) | Error::Service(ServiceError::ZBus(z)) => Some(z),
        _ => None,
    };
    match wire {
        Some(z) if is_unknown_service(z) => {
            "no Secret Service is running; History encryption keeps its key there \
             (start gnome-keyring, KeePassXC's Secret Service integration or oo7-daemon)"
                .into()
        }
        Some(
            zbus::Error::Address(_) | zbus::Error::InputOutput(_) | zbus::Error::Connection(..),
        ) => {
            format!("cannot reach the session bus for the Secret Service: {e}")
        }
        _ if matches!(e, Error::Service(ServiceError::IsLocked(_))) => LOCKED.into(),
        _ => format!("the Secret Service failed: {e}"),
    }
}

fn is_unknown_service(e: &zbus::Error) -> bool {
    const NAMES: [&str; 2] = [
        "org.freedesktop.DBus.Error.ServiceUnknown",
        "org.freedesktop.DBus.Error.NameHasNoOwner",
    ];
    match e {
        zbus::Error::MethodError(name, ..) => NAMES.contains(&name.as_str()),
        zbus::Error::FDO(e) => NAMES.contains(&zbus::DBusError::name(&**e).as_str()),
        _ => false,
    }
}
