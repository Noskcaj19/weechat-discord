use crate::{
    buffer::{ext::BufferExt, guild::Guild, pins::Pins},
    config::{Config, GuildConfig},
    discord::discord_connection::DiscordConnection,
    instance::Instance,
    twilight_utils::ext::UserExt,
};
use std::{borrow::Cow, sync::Arc};
use twilight_cache_inmemory::model::CachedGuild;
use twilight_model::channel::GuildChannel;
use weechat::{
    buffer::Buffer,
    hooks::{Command, CommandRun, CommandSettings},
    Args, ReturnCode, Weechat,
};
use weechat_command_parser::{Command as WeechatCommand, ParsedCommand};

pub struct Commands {
    pub _discord_command: Command,
    pub _me_hook: CommandRun,
}

pub struct DiscordCommand {
    instance: Instance,
    connection: DiscordConnection,
    config: Config,
}

impl DiscordCommand {
    fn add_guild(&self, matches: ParsedCommand) {
        // TODO: Abstract guild resolution code
        let cache = match self.connection.borrow().as_ref() {
            Some(conn) => conn.cache.clone(),
            None => {
                Weechat::print("discord: must be connected to add servers");
                return;
            },
        };
        let guild_name = matches
            .arg("name")
            .expect("name is required by verification")
            .to_owned();

        {
            let config = self.config.clone();
            let instance = self.instance.clone();
            if let Some(conn) = self.connection.borrow().clone() {
                Weechat::spawn(async move {
                    match crate::twilight_utils::search_cached_striped_guild_name(
                        &cache,
                        &guild_name,
                    ) {
                        Some(guild) => {
                            let mut config_borrow = config.config.borrow_mut();
                            let mut section = config_borrow
                                .search_section_mut("server")
                                .expect("Can't get server section");

                            if instance.borrow_guilds().contains_key(&guild.id) {
                                tracing::info!(
                                    %guild.id,
                                    %guild.name,
                                    "Guild not added to config, already exists.",
                                );
                                Weechat::print(&format!(
                                    "discord: \"{}\" has already been added",
                                    guild.name
                                ));
                            } else {
                                tracing::info!(%guild.id, %guild.name, "Adding guild to config.");
                                Weechat::print(&format!("discord: added \"{}\"", guild.name));
                                Guild::try_create(
                                    Arc::clone(&guild),
                                    &instance,
                                    &conn,
                                    GuildConfig::new(&mut section, guild.id),
                                    &config,
                                );
                            }
                        },
                        None => {
                            tracing::info!("Could not find guild: \"{}\"", guild_name);
                            Weechat::print(&format!(
                                "discord: could not find guild: {}",
                                guild_name
                            ));
                        },
                    };
                })
                .detach();
            }
        }
    }

    fn remove_guild(&self, matches: ParsedCommand) {
        let cache = match self.connection.borrow().as_ref() {
            None => {
                Weechat::print("discord: must be connected to remove servers");
                return;
            },
            Some(conn) => conn.cache.clone(),
        };
        let guild_name = matches
            .arg("name")
            .expect("name is required by verification")
            .to_owned();

        {
            let instance = self.instance.clone();
            Weechat::spawn(async move {
                let guild_ids = instance.borrow_guilds().keys().copied().collect::<Vec<_>>();
                match crate::twilight_utils::search_striped_guild_name(
                    &cache,
                    guild_ids,
                    &guild_name,
                ) {
                    Some(guild) => {
                        if instance.borrow_guilds_mut().remove(&guild.id).is_some() {
                            tracing::info!(%guild.id, %guild.name, "Removed guild from config.");
                            Weechat::print(&format!("discord: removed server \"{}\"", guild.name));
                        } else {
                            tracing::info!(%guild.id, %guild.name, "Guild not added.");
                            Weechat::print(&format!(
                                "discord: server \"{}\" not in config",
                                guild.name
                            ));
                        }
                    },
                    None => {
                        tracing::info!("discord: Could not find guild: \"{}\"", guild_name);
                        Weechat::print(&format!("discord: could not find server: {}", guild_name));
                    },
                };
            })
            .detach();
        }
    }

    fn list_guilds(&self) {
        Weechat::print("discord: servers:");

        if let Some(connection) = self.connection.borrow().as_ref() {
            let cache = connection.cache.clone();
            for (guild_id, guild_) in self.instance.borrow_guilds().clone().into_iter() {
                let cache = cache.clone();
                Weechat::spawn(async move {
                    let guild = cache.guild(guild_id);
                    if let Some(guild) = guild {
                        Weechat::print(&format!("{}{}", Weechat::color("chat_server"), guild.name));
                    } else {
                        Weechat::print(&format!("{:?}", guild_id));
                    }

                    Weechat::print(" Autojoin channels:");
                    for channel_id in guild_.guild_config.autojoin_channels().iter() {
                        if let Some(channel) = cache.guild_channel(*channel_id) {
                            Weechat::print(&format!("  #{}", channel.name()));
                        } else {
                            Weechat::print(&format!("  #{:?}", channel_id));
                        }
                    }
                    Weechat::print(" Watched channels:");
                    for channel_id in guild_.guild_config.watched_channels().iter() {
                        if let Some(channel) = cache.guild_channel(*channel_id) {
                            Weechat::print(&format!("  #{}", channel.name()));
                        } else {
                            Weechat::print(&format!("  #{:?}", channel_id));
                        }
                    }
                })
                .detach();
            }
        } else {
            for (guild_id, guild) in self.instance.borrow_guilds().clone().into_iter() {
                Weechat::print(&format!("{:?}", guild_id));
                Weechat::print(" Autojoin channels:");
                for channel_id in guild.guild_config.autojoin_channels() {
                    Weechat::print(&format!("  #{:?}", channel_id));
                }
                Weechat::print(" Watched channels:");
                for channel_id in guild.guild_config.watched_channels() {
                    Weechat::print(&format!("  #{:?}", channel_id));
                }
            }
        }
    }

    fn autoconnect_guild(&self, matches: ParsedCommand) {
        let guild_name = matches
            .arg("name")
            .expect("name is required by verification")
            .to_owned();

        let instance = self.instance.clone();
        let connection = self.connection.clone();
        Weechat::spawn(async move {
            let cache = match connection.borrow().as_ref() {
                Some(conn) => conn.cache.clone(),
                None => {
                    Weechat::print("discord: must be connected to enable server autoconnect");
                    return;
                },
            };

            let guilds = instance.borrow_guilds().keys().copied().collect::<Vec<_>>();
            match crate::twilight_utils::search_striped_guild_name(&cache, guilds, &guild_name) {
                Some(guild) => {
                    let weechat_guild = instance.borrow_guilds().get(&guild.id).cloned();
                    if let Some(weechat_guild) = weechat_guild {
                        tracing::info!(%guild.id, %guild.name, "Enabled autoconnect for guild");
                        weechat_guild.guild_config.set_autoconnect(true);
                        weechat_guild.guild_config.persist(&weechat_guild.config);
                        Weechat::print(&format!(
                            "discord: now autoconnecting to server \"{}\"",
                            guild.name
                        ));
                        weechat_guild.try_join_channels();
                    } else {
                        tracing::info!(%guild.id, %guild.name, "Guild not added.");
                        Weechat::print(&format!(
                            "discord: server \"{}\" not in config",
                            guild.name
                        ));
                    }
                },
                None => {
                    tracing::info!("Could not find guild: \"{}\"", guild_name);
                    Weechat::print(&format!("discord: could not find server: {}", guild_name));
                },
            };
        })
        .detach();
    }

    fn noautoconnect_guild(&self, matches: ParsedCommand) {
        let guild_name = matches
            .arg("name")
            .expect("name is required by verification")
            .to_owned();

        let instance = self.instance.clone();
        let connection = self.connection.clone();
        Weechat::spawn(async move {
            let cache = match connection.borrow().as_ref() {
                Some(conn) => conn.cache.clone(),
                None => {
                    Weechat::print("discord: must be connected to enable server autoconnect");
                    return;
                },
            };

            match crate::twilight_utils::search_striped_guild_name(
                &cache,
                instance.borrow_guilds().keys().copied(),
                &guild_name,
            ) {
                Some(guild) => {
                    if let Some(weechat_guild) = instance.borrow_guilds().get(&guild.id) {
                        tracing::info!(%guild.id, %guild.name, "Disabled autoconnect for guild");
                        weechat_guild.guild_config.set_autoconnect(false);
                        weechat_guild.guild_config.persist(&weechat_guild.config);
                        Weechat::print(&format!(
                            "discord: no longer autoconnecting to server \"{}\"",
                            guild.name
                        ));
                    } else {
                        tracing::info!(%guild.id, %guild.name, "Guild not added.");
                        Weechat::print(&format!(
                            "discord: server \"{}\" not in config",
                            guild.name
                        ));
                    }
                },
                None => {
                    tracing::info!("Could not find guild: \"{}\"", guild_name);
                    Weechat::print(&format!(
                        "discord: could not find server: \"{}\"",
                        guild_name
                    ));
                },
            };
        })
        .detach();
    }

    fn process_server_matches(&self, matches: ParsedCommand) {
        match matches.subcommand() {
            Some(("add", matches)) => self.add_guild(matches),
            Some(("remove", matches)) => self.remove_guild(matches),
            Some(("list", _)) => self.list_guilds(),
            Some(("autoconnect", matches)) => self.autoconnect_guild(matches),
            Some(("noautoconnect", matches)) => self.noautoconnect_guild(matches),
            _ => unreachable!("Reached subcommand that does not exist in clap config"),
        }
    }

    fn add_autojoin_channel(&self, matches: ParsedCommand) {
        if let Some((_, weecord_guild, channel)) = self.resolve_channel_and_guild(matches) {
            weecord_guild
                .guild_config
                .autojoin_channels_mut()
                .push(channel.id());
            weecord_guild.guild_config.persist(&weecord_guild.config);
            tracing::info!(%weecord_guild.id, channel.id=%channel.id(), "Added channel to autojoin list");
            Weechat::print(&format!(
                "discord: added channel \"{}\" to autojoin list",
                channel.name()
            ));

            let _ = weecord_guild.join_channel(&channel);
        }
    }

    fn remove_autojoin_channel(&self, matches: ParsedCommand) {
        if let Some((guild, weecord_guild, channel)) = self.resolve_channel_and_guild(matches) {
            {
                let mut autojoin = weecord_guild.guild_config.autojoin_channels_mut();
                if let Some(pos) = autojoin.iter().position(|x| *x == channel.id()) {
                    autojoin.remove(pos);
                    tracing::info!(%weecord_guild.id, channel.id=%channel.id(), "Removed channel from autojoin list");
                    Weechat::print(&format!(
                        "discord: removed channel \"{}\" from autojoin list",
                        guild.name
                    ));
                }
            }
            weecord_guild.guild_config.persist(&weecord_guild.config);
        }
    }

    fn join_channel(&self, matches: ParsedCommand) {
        if let Some((_, weecord_guild, channel)) = self.resolve_channel_and_guild(matches) {
            Weechat::spawn(async move {
                if let Err(e) = weecord_guild.join_channel(&channel) {
                    Weechat::print(&format!("discord: unable to join channel \"{}\"", e));
                }
            })
            .detach();
        }
    }

    fn resolve_channel_and_guild(
        &self,
        matches: ParsedCommand,
    ) -> Option<(Arc<CachedGuild>, Guild, Arc<GuildChannel>)> {
        let guild_name = matches
            .arg("guild_name")
            .expect("guild name is enforced by verification")
            .to_owned();
        let channel_name = matches
            .arg("name")
            .expect("channel name is enforced by verification")
            .to_owned();

        let connection = self.connection.borrow();
        let connection = match connection.as_ref() {
            Some(conn) => conn,
            None => {
                Weechat::print("discord: must be connected to join channels");
                return None;
            },
        };

        let instance = self.instance.clone();
        let cache = connection.cache.clone();

        let result = if let Some(guild) =
            crate::twilight_utils::search_cached_striped_guild_name(&cache, &guild_name)
        {
            tracing::trace!(%guild.name, "Matched guild");
            if let Some(channel) = crate::twilight_utils::search_cached_stripped_guild_channel_name(
                &cache,
                guild.id,
                &channel_name,
            ) {
                tracing::trace!("Matched channel {}", channel.name());
                Ok((guild, channel))
            } else {
                tracing::warn!(%channel_name, "Unable to find matching channel");
                Err(anyhow::anyhow!(
                    "could not find channel: \"{}\"",
                    channel_name
                ))
            }
        } else {
            tracing::warn!(%channel_name, "Unable to find matching guild: \"{}\"", guild_name);
            Err(anyhow::anyhow!("could not find server: \"{}\"", guild_name))
        };

        match result {
            Ok((guild, channel)) => {
                if let Some(weecord_guild) =
                    instance.borrow_guilds().values().find(|g| g.id == guild.id)
                {
                    Some((guild, weecord_guild.clone(), channel))
                } else {
                    tracing::warn!(%guild.id, "Guild has not been added to weechat");
                    Weechat::spawn_from_thread(async move {
                        Weechat::print(&format!(
                            "discord: could not find server \"{}\" in config",
                            guild.name
                        ));
                    });
                    None
                }
            },
            Err(e) => {
                Weechat::spawn_from_thread(async move {
                    Weechat::print(&format!("{}", e));
                });
                None
            },
        }
    }

    fn process_channel_matches(&self, matches: ParsedCommand) {
        match matches.subcommand() {
            Some(("autojoin", matches)) => self.add_autojoin_channel(matches),
            Some(("noautojoin", matches)) => self.remove_autojoin_channel(matches),
            Some(("join", matches)) => self.join_channel(matches),
            _ => {},
        }
    }

    fn token(&self, matches: ParsedCommand) {
        let token = matches.arg("token").expect("enforced by validation");

        self.config.borrow_inner_mut().token = Some(token.trim().trim_matches('"').to_owned());
        self.config.persist();

        Weechat::print("discord: updated token");
        tracing::info!("Updated discord token");
    }

    fn query(&self, matches: ParsedCommand) {
        let user = matches.arg("user").expect("enforced by validation");

        let conn = self.connection.borrow();
        let conn = match conn.as_ref() {
            Some(conn) => conn,
            None => {
                Weechat::print("discord: must be connected to join channels");
                return;
            },
        };

        for channel in conn.cache.private_channels().expect("always returns Some") {
            let name = channel
                .recipients
                .iter()
                .map(|u| crate::utils::clean_name_with_case(&u.tag()))
                .collect::<Vec<_>>()
                .join(",");

            if name == user {
                let config = self.config.clone();
                let conn = conn.clone();
                let instance = self.instance.clone();
                Weechat::spawn(async move {
                    let _ = DiscordConnection::create_private_channel(
                        &conn, &config, &instance, &channel,
                    );
                })
                .detach();
                return;
            }
        }
        Weechat::print(&format!("discord: Unable to find user \"{}\"", user));
        tracing::info!("Unable to find user \"{}\"", user);
    }

    fn pins(&self, weechat: &Weechat) {
        let conn = self.connection.borrow();
        let conn = match conn.as_ref() {
            Some(conn) => conn.clone(),
            None => {
                Weechat::print("discord: must be connected to view pinned messages");
                return;
            },
        };

        let buffer = weechat.current_buffer();
        let guild_id = buffer.guild_id();
        let channel_id = buffer.channel_id();
        let config = self.config.clone();
        let instance = self.instance.clone();
        Weechat::spawn(async move {
            let pins = Pins::new(guild_id, channel_id.unwrap(), conn, &config);

            if let Err(e) = pins.load().await {
                tracing::error!(
                    guild.id=?guild_id,
                    channel.id=?channel_id,
                    "Unable to load pins: {}",
                    e
                );

                Weechat::print(&format!(
                    "discord: an error occurred loading channel pins: {}",
                    e
                ));
            };

            instance
                .borrow_pins_mut()
                .insert((guild_id.unwrap(), channel_id.unwrap()), pins);
        })
        .detach();
    }

    fn more_history(&self, buffer: &Buffer) {
        if let Some(channel_id) = buffer.channel_id() {
            if let Some(channel) = self.instance.search_buffer(buffer.guild_id(), channel_id) {
                Weechat::spawn(async move {
                    if let Err(e) = channel.load_history().await {
                        tracing::error!("Failed to load more history: {}", e);
                    }
                })
                .detach();
                return;
            }
        }
        Weechat::print("discord: Not a Discord buffer");
        tracing::warn!(
            buffer.name = buffer.name().to_string().as_str(),
            "Unable to find Discord channel"
        );
    }

    fn discord_format(&self, matches: ParsedCommand, weechat: &Weechat, raw: &str) {
        let conn = self.connection.borrow();
        let conn = match conn.as_ref() {
            Some(conn) => conn.clone(),
            None => {
                Weechat::print("discord: must be connected to view pinned messages");
                return;
            },
        };

        let cmd = matches.command();
        let msg = matches.rest(raw).trim_start();

        let msg = match cmd {
            "me" => format!("_{}_", msg),
            "tableflip" => format!("{} (╯°□°）╯︵ ┻━┻", msg),
            "unflip" => format!("{} ┬─┬ ノ( ゜-゜ノ)", msg),
            "shrug" => format!("{} ¯\\_(ツ)_/¯", msg),
            "spoiler" => format!("||{}||", msg),
            _ => unreachable!(),
        };

        let buffer = weechat.current_buffer();
        let channel_id = buffer.channel_id();

        if let Some(channel_id) = channel_id {
            let http = conn.http.clone();
            conn.rt.spawn(async move {
                let future = match http.create_message(channel_id).content(msg) {
                    Ok(future) => future,
                    Err(e) => {
                        tracing::error!(
                            channel.id = channel_id.0,
                            "an error occurred creating message: {}",
                            e
                        );
                        Weechat::print("discord: the message is too long");
                        return;
                    },
                };
                if let Err(e) = future.await {
                    tracing::error!(
                        channel.id = channel_id.0,
                        "an error occurred sending message in this channel: {}",
                        e
                    );
                    Weechat::print("discord: an error occurred sending the message");
                }
            });
        } else {
            tracing::warn!(
                "buffer has no associated channel id: {}",
                buffer.full_name()
            );
            Weechat::print("discord: this is not a discord buffer");
        }
    }

    fn process_debug_matches(&self, matches: ParsedCommand, weechat: &Weechat) {
        match matches.subcommand() {
            Some(("buffer", _)) => {
                Weechat::print(&format!(
                    "Guild id: {:?}",
                    weechat.current_buffer().guild_id()
                ));
                Weechat::print(&format!(
                    "Channel id: {:?}",
                    weechat.current_buffer().channel_id()
                ));
            },
            Some(("buffers", _)) => {
                for guild in self.instance.borrow_guilds().values() {
                    let (strng, weak) = guild.debug_counts();
                    Weechat::print(&format!("Guild [{} {}]: {}", strng, weak, guild.id));

                    for channel in guild.channels().values() {
                        Weechat::print(&format!("  Channel: {}", channel.id));
                    }
                }

                for private_channel in self.instance.borrow_private_channels().values() {
                    let (strng, weak) = private_channel.debug_counts();

                    Weechat::print(&format!(
                        "Private Channel [{} {}]: {}",
                        strng, weak, private_channel.id
                    ));
                }

                for pins in self.instance.borrow_pins_mut().values() {
                    let (strng, weak) = pins.debug_counts();

                    Weechat::print(&format!(
                        "Pin Channel [{} {}]: {:?} {}",
                        strng, weak, pins.guild_id, pins.channel_id
                    ));
                }
            },
            Some(("members", _)) => {
                let conn = self.connection.borrow();
                let conn = match conn.as_ref() {
                    Some(conn) => conn.clone(),
                    None => {
                        Weechat::print("discord: must be connected to view guild members messages");
                        return;
                    },
                };

                let buffer = weechat.current_buffer();
                let guild_id = buffer.guild_id();
                if let Some(members) = conn.cache.guild_members(guild_id.unwrap()) {
                    for user_id in members {
                        Weechat::print(&format!(
                            "{}: {:?}",
                            user_id,
                            conn.cache.member(guild_id.unwrap(), user_id)
                        ));
                    }
                }
            },
            Some(("shutdown", _)) => {
                self.connection.shutdown();
                self.instance.borrow_guilds_mut().clear();
                self.instance.borrow_private_channels_mut().clear();
                self.instance.borrow_pins_mut().clear();
            },
            _ => {},
        }
    }
}

impl weechat::hooks::CommandCallback for DiscordCommand {
    fn callback(&mut self, weechat: &Weechat, buffer: &Buffer, arguments: Args) {
        let args = arguments.collect::<Vec<_>>();

        let matches = WeechatCommand::new("/discord")
            .subcommand(
                WeechatCommand::new("server")
                    .subcommand(WeechatCommand::new("add").arg("name", true))
                    .subcommand(WeechatCommand::new("remove").arg("name", true))
                    .subcommand(WeechatCommand::new("autoconnect").arg("name", true))
                    .subcommand(WeechatCommand::new("noautoconnect").arg("name", true))
                    .subcommand(WeechatCommand::new("list")),
            )
            .subcommand(
                WeechatCommand::new("channel")
                    .subcommand(
                        WeechatCommand::new("autojoin")
                            .arg("guild_name", true)
                            .arg("name", true),
                    )
                    .subcommand(
                        WeechatCommand::new("noautojoin")
                            .arg("guild_name", true)
                            .arg("name", true),
                    )
                    .subcommand(
                        WeechatCommand::new("join")
                            .arg("guild_name", true)
                            .arg("name", true),
                    ),
            )
            .subcommand(WeechatCommand::new("query").arg("user", true))
            .subcommand(
                WeechatCommand::new("debug")
                    .subcommand(WeechatCommand::new("buffer"))
                    .subcommand(WeechatCommand::new("buffers"))
                    .subcommand(WeechatCommand::new("shutdown"))
                    .subcommand(WeechatCommand::new("members")),
            )
            .subcommand(WeechatCommand::new("token").arg("token", true))
            .subcommand(WeechatCommand::new("pins"))
            .subcommand(WeechatCommand::new("more_history"))
            .subcommand(WeechatCommand::new("me"))
            .subcommand(WeechatCommand::new("tableflip"))
            .subcommand(WeechatCommand::new("unflip"))
            .subcommand(WeechatCommand::new("shrug"))
            .subcommand(WeechatCommand::new("spoiler"))
            .parse_from(args.iter());

        let matches = match matches {
            Ok(matches) => {
                tracing::trace!("{:#?}", matches);
                matches
            },
            Err(err) => {
                tracing::error!("Error parsing command: \"{:?}\" {:?}", args, err);
                Weechat::print(&format!("discord: {}", err));
                return;
            },
        };

        match matches.subcommand() {
            Some(("server", matches)) => self.process_server_matches(matches),
            Some(("channel", matches)) => self.process_channel_matches(matches),
            Some(("token", matches)) => self.token(matches),
            Some(("query", matches)) => self.query(matches),
            Some(("pins", _)) => self.pins(weechat),
            Some(("more_history", _)) => self.more_history(buffer),
            // Use or-patterns when they stabilize (rust #54883)
            Some(("me", matches))
            | Some(("tableflip", matches))
            | Some(("unflip", matches))
            | Some(("shrug", matches))
            | Some(("spoiler", matches)) => self.discord_format(matches, weechat, &args.join(" ")),
            Some(("debug", matches)) => self.process_debug_matches(matches, weechat),
            _ => {},
        };
    }
}

pub fn hook(connection: DiscordConnection, instance: Instance, config: Config) -> Commands {
    let _discord_command = Command::new(
        CommandSettings::new("discord")
            .description("Discord integration for weechat")
            .add_argument("token <token>")
            .add_argument("server add|remove|list|autoconnect|noautoconnect <server-name>")
            .add_argument("channel join|autojoin|noautojoin <server-name> <channel-name>")
            .add_argument("query <user-name>")
            .add_argument("pins")
            .add_argument("more_history")
            .add_argument("me|tableflip|unflip|shrug|spoiler")
            .add_argument("debug buffer|buffers|shutdown|members")
            .add_completion("token")
            .add_completion("server add|remove|list|autoconnect|noautoconnect %(discord_guild)")
            .add_completion("channel join|autojoin|noautojoin %(discord_guild) %(discord_channel)")
            .add_completion("query %(discord_dm)")
            .add_completion("pins")
            .add_completion("more_history")
            .add_completion("me|tableflip|unflip|shrug|spoiler")
            .add_completion("debug buffer|shutdown|members"),
        DiscordCommand {
            instance,
            connection,
            config,
        },
    )
    .expect("Failed to create command");

    let _me_hook = CommandRun::new("/me", |_: &Weechat, buffer: &Buffer, command: Cow<str>| {
        if let Some(text) = command.splitn(2, ' ').nth(1) {
            let string = format!("/discord me {}", text.trim_start());
            let _ = buffer.run_command(&string);
        }
        ReturnCode::Ok
    })
    .expect("Unable to hook me command run");

    Commands {
        _discord_command,
        _me_hook,
    }
}
