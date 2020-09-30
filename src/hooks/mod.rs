use crate::{config::Config, discord::discord_connection::DiscordConnection, instance::Instance};
use weechat::Weechat;

mod command;
mod completions;
mod options;
mod signals;

pub use completions::Completions;
pub use options::Options;
pub use signals::Signals;
pub use weechat::hooks::Command;

pub struct Hooks {
    _completions: Completions,
    _command: Command,
    _options: Options,
    _signals: Signals,
}

impl Hooks {
    pub fn hook_all(
        weechat: &Weechat,
        discord_connection: DiscordConnection,
        instance: Instance,
        config: Config,
    ) -> Hooks {
        let _command = command::hook(discord_connection.clone(), instance.clone(), config.clone());
        tracing::trace!("Command hooked");

        let _completions = Completions::hook_all(discord_connection);
        tracing::trace!("Completions hooked");

        let _options = Options::hook_all(weechat, config);
        tracing::trace!("Options hooked");

        let _signals = Signals::hook_all(instance);
        tracing::trace!("Signals hooked");

        Hooks {
            _completions,
            _command,
            _options,
            _signals,
        }
    }
}