use pumpkin_plugin_api::{Context, Plugin, PluginMetadata};

struct VoteKin;

impl Plugin for VoteKin {
    fn new() -> Self {
        Self
    }

    fn metadata(&self) -> PluginMetadata {
        PluginMetadata {
            name: "votekin".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            authors: vec!["Jonny Bogarin".into()],
            description: "Vote listener for Pumpkin servers".into(),
            dependencies: vec![],
            permissions: vec![],
        }
    }

    fn on_load(&self, _context: Context) -> Result<(), String> {
        tracing::info!("VoteKin {} loaded", env!("CARGO_PKG_VERSION"));
        tracing::info!("Vote reception is not implemented yet");
        Ok(())
    }

    fn on_unload(&self, _context: Context) -> Result<(), String> {
        tracing::info!("VoteKin unloaded");
        Ok(())
    }
}

pumpkin_plugin_api::register_plugin!(VoteKin);
