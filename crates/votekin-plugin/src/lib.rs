mod config;
mod legacy;
mod listener;
mod subscriptions;

use listener::{Event, Listener};
use pumpkin_plugin_api::{
    Context, Plugin, PluginMetadata, ipc, permissions,
    scheduler::{self, SchedulerExt},
};
use std::{
    path::Path,
    sync::{Arc, Mutex},
};

struct VoteKin {
    listener: Arc<Mutex<Option<Listener>>>,
    task: Mutex<Option<u32>>,
    subscriptions: Arc<subscriptions::Subscriptions>,
}

impl Plugin for VoteKin {
    fn new() -> Self {
        Self {
            listener: Arc::new(Mutex::new(None)),
            task: Mutex::new(None),
            subscriptions: Arc::new(subscriptions::Subscriptions::default()),
        }
    }

    fn metadata(&self) -> PluginMetadata {
        PluginMetadata {
            name: "votekin".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            authors: vec!["Jonny Bogarin".into()],
            description: "Vote listener for Pumpkin servers".into(),
            dependencies: vec![],
            permissions: vec![
                permissions::NETWORK_TCP_BIND.into(),
                permissions::FS_READ_DATA.into(),
                permissions::FS_WRITE_DATA.into(),
            ],
        }
    }

    fn on_load(&self, context: Context) -> Result<(), String> {
        let config = config::Config::load(Path::new(&context.get_data_folder()))?;
        let address = config.address();
        let enable_v1 = config.enable_v1;
        let legacy = if enable_v1 {
            Some(legacy::Legacy::load(Path::new(&context.get_data_folder()))?)
        } else {
            None
        };
        let listener = Listener::bind(config, legacy)?;
        *self
            .listener
            .lock()
            .map_err(|_| "VoteKin listener lock failed")? = Some(listener);
        let shared = Arc::clone(&self.listener);
        let subscriptions = Arc::clone(&self.subscriptions);
        let task = context.schedule_repeating_task(1, 1, move |_| {
            let events = {
                let Ok(mut state) = shared.try_lock() else {
                    return;
                };
                state.as_mut().map(Listener::poll).unwrap_or_default()
            };
            for event in events {
                match event {
                    Event::Accepted(vote) => {
                        tracing::info!(service = ?vote.service(), username = ?vote.username(), protocol = ?vote.source_protocol(), "Vote received");
                        match subscriptions.publish(&vote, |recipient, message| {
                            match ipc::send_ipc_message(recipient, message) {
                                Ok(Ok(_)) => Ok(()),
                                _ => Err(()),
                            }
                        }) {
                            Ok(failed) => for consumer in failed {
                                tracing::warn!(consumer = ?consumer, "Vote delivery failed: consumer unavailable or returned an error");
                            },
                            Err(error) => tracing::warn!(%error, "Vote delivery failed"),
                        }
                    }
                    Event::Failure { reason, suppressed } => tracing::warn!(
                        %reason, suppressed, "VoteKin connection rejected or failed"
                    ),
                }
            }
        });
        *self.task.lock().map_err(|_| "VoteKin task lock failed")? = Some(task);
        tracing::info!("VoteKin {} loaded", env!("CARGO_PKG_VERSION"));
        tracing::info!(%address, enable_v1, "VoteKin listening (NuVotifier v2)");
        if enable_v1 {
            tracing::warn!(
                "Legacy v1 enabled: votes are not authenticated; public key is in plugins/data/votekin/public.key"
            );
        }
        tracing::info!("Votes are forwarded to subscribed plugins; VoteKin does not issue rewards");
        Ok(())
    }

    fn handle_ipc_message(&self, sender: String, message: Vec<u8>) -> Result<Vec<u8>, String> {
        let response = self.subscriptions.handle(&sender, &message)?;
        tracing::debug!(consumer = ?sender, "VoteKin subscription request handled");
        Ok(response)
    }

    fn on_unload(&self, _context: Context) -> Result<(), String> {
        let task = self
            .task
            .lock()
            .map_err(|_| "VoteKin task lock failed")?
            .take();
        if let Some(task) = task {
            scheduler::cancel_task(task);
        }
        self.listener
            .lock()
            .map_err(|_| "VoteKin listener lock failed")?
            .take();
        self.subscriptions.clear()?;
        tracing::info!("VoteKin listener stopped");
        tracing::info!("VoteKin unloaded");
        Ok(())
    }
}

pumpkin_plugin_api::register_plugin!(VoteKin);
