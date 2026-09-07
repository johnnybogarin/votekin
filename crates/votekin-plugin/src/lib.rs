use pumpkin_plugin_api::{Plugin, PluginMetadata};

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
}

pumpkin_plugin_api::register_plugin!(VoteKin);
