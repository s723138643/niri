use calloop::{EventSource, Interest, LoopHandle, RegistrationToken};
use smithay::backend::allocator::dmabuf::Dmabuf;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::{Client, Resource};
use smithay::wayland::compositor::{add_blocker, Blocker, CompositorHandler as _};

use crate::niri::State;
use crate::utils::transaction::Transaction;

pub fn add_dmabuf_blocker(
    surface: &WlSurface,
    dmabuf: Option<Dmabuf>,
    event_loop: &LoopHandle<State>,
    transaction: Option<Transaction>,
) {
    let Some(dmabuf) = dmabuf else {
        return;
    };

    let Some(client) = surface.client() else {
        return;
    };

    let (blocker, source) = match dmabuf.generate_blocker(Interest::READ) {
        Ok(res) => res,
        Err(_) => {
            return;
        }
    };

    let res = add_blocker_and_source(surface, client, blocker, source, transaction, event_loop);
    match res {
        Ok(_) => trace!("added dmabuf blocker"),
        Err(e) => warn!("add dmabuf blocker failed, error: {e:?}"),
    };
}

fn add_blocker_and_source<B, S>(
    surface: &WlSurface,
    client: Client,
    blocker: B,
    source: S,
    mut transaction: Option<Transaction>,
    event_loop: &LoopHandle<State>,
) -> Result<RegistrationToken, std::io::Error>
where
    S: EventSource<Ret = Result<(), std::io::Error>> + 'static,
    B: Blocker + Send + 'static,
{
    let res = event_loop
        .insert_source(source, move |_, _, state| {
            // This surface is now ready for the transaction.
            drop(transaction.take());

            let display_handle = state.niri.display_handle.clone();
            state
                .client_compositor_state(&client)
                .blocker_cleared(state, &display_handle);

            Ok(())
        })
        .map_err(|e| e.error)?;
    add_blocker(surface, blocker);
    Ok(res)
}
