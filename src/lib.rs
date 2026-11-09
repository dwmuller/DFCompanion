pub mod ai;
pub mod ai_config;
pub mod ai_providers;
pub mod ai_tools;
pub mod ai_usage;
pub mod app;
pub mod chat_commands;
pub mod diagnostics;
pub mod game_state;
pub mod notifications;

pub mod api {
    tonic::include_proto!("dfcompanion.v1");
}
pub mod rpc;
pub mod web;
