use crate::{
    buffer::{channel::Channel, ext::BufferExt, guild::Guild},
    config::{Config, GuildConfig},
    discord::{plugin_message::PluginMessage, typing_indicator::TypingEntry},
    instance::Instance,
    refcell::{Ref, RefCell},
    twilight_utils::ext::{ChannelExt, MemberExt, UserExt},
};
use anyhow::Result;
use futures::StreamExt;
use once_cell::sync::Lazy;
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::{
    runtime::Runtime,
    sync::{
        mpsc::{Receiver, Sender},
        oneshot::channel,
        Mutex as TokioMutex,
    },
};
use twilight_cache_inmemory::InMemoryCache;
use twilight_gateway::{shard::ShardStartErrorType, Event as GatewayEvent, Intents, Shard};
use twilight_http::{error::ErrorType as HttpErrorType, Client as HttpClient};
use twilight_model::{
    channel::PrivateChannel,
    id::{ChannelId, GuildId},
};
use weechat::Weechat;

#[derive(Clone, Debug)]
pub struct ConnectionInner {
    pub shard: Shard,
    pub rt: Arc<Runtime>,
    pub cache: InMemoryCache,
    pub http: HttpClient,
}

#[derive(Clone)]
pub struct DiscordConnection(Arc<RefCell<Option<ConnectionInner>>>);

impl DiscordConnection {
    pub fn new() -> Self {
        Self(Arc::new(RefCell::new(None)))
    }

    pub fn borrow(&self) -> Ref<'_, Option<ConnectionInner>> {
        self.0.borrow()
    }

    pub async fn start(&self, token: &str, tx: Sender<PluginMessage>) -> Result<ConnectionInner> {
        let (cache_tx, cache_rx) = channel();
        let runtime = Arc::new(Runtime::new().expect("Unable to create tokio runtime"));
        let token = token.to_owned();
        {
            let tx = tx.clone();
            let rt = runtime.clone();
            runtime.spawn(async move {
                let mut shard = Shard::new(&token, Intents::all());
                if let Err(e) = shard.start().await {
                    let err_msg = format!("An error occurred connecting to Discord: {}", e);
                    Weechat::spawn_from_thread(async move {
                        Weechat::print(&err_msg);
                    });

                    tracing::error!("An error occurred connecting to Discord: {:#?}", e);

                    // Check if the error is a 401 Unauthorized, which is likely an invalid token
                    if let ShardStartErrorType::RetrievingGatewayUrl = e.kind() {
                        if let Some(e) = e
                            .into_source()
                            .and_then(|s| s.downcast::<twilight_http::error::Error>().ok())
                        {
                            if let HttpErrorType::Response { status, .. } = e.kind() {
                                if status.as_u16() == 401 {
                                    Weechat::spawn_from_thread(async move {
                                        Weechat::print(
                                            "discord: unauthorized: check that your token is valid",
                                        );
                                    });
                                }
                            }
                        }
                    }
                    return;
                };

                let shard = shard;

                rt.spawn({
                    let shard = shard.clone();
                    async move {
                        fn waiting(shard: &Shard) -> bool {
                            match shard.info().map(|info| info.stage()) {
                                Ok(twilight_gateway::shard::Stage::Connected) => false,
                                Ok(_) => true,
                                Err(_) => true,
                            }
                        }
                        tokio::time::sleep(Duration::from_secs(7)).await;

                        if waiting(&shard) {
                            Weechat::spawn_from_thread(async {
                                Weechat::print(
                                    "discord: Still waiting for Ready from Discord gateway",
                                );
                            });
                        }
                        tokio::time::sleep(Duration::from_secs(13)).await;

                        if waiting(&shard) {
                            Weechat::spawn_from_thread(async {
                                Weechat::print(
                                    "discord: Gateway still not successfully connected...  there \
                                     is likely an issue with Discord or weecord, see logs for \
                                     more details",
                                );
                            });
                        }
                    }
                });

                let cache = InMemoryCache::new();

                tracing::info!("Connected to Discord, waiting for Ready...");
                Weechat::spawn_from_thread(async {
                    Weechat::print("discord: connected, waiting for ready...");
                });
                let mut events = shard.events();

                let http = shard.config().http_client();
                cache_tx
                    .send((shard.clone(), cache.clone(), http.clone()))
                    .map_err(|_| ())
                    .expect("Cache receiver closed before data could be sent");

                while let Some(event) = events.next().await {
                    cache.update(&event);

                    tokio::spawn(Self::handle_gateway_event(event, tx.clone()));
                }
            });
        }

        let (shard, cache, http) = cache_rx
            .await
            .map_err(|_| anyhow::anyhow!("The connection to discord failed"))?;

        let meta = ConnectionInner {
            shard,
            rt: runtime,
            cache,
            http,
        };

        self.0.borrow_mut().replace(meta.clone());

        Ok(meta)
    }

    pub fn shutdown(&self) {
        if let Some(inner) = self.0.borrow_mut().take() {
            inner.shard.shutdown();
        }
    }

    pub async fn send_guild_subscription(&self, guild_id: GuildId, channel_id: ChannelId) {
        let inner = self.0.borrow().as_ref().cloned();
        if let Some(inner) = inner {
            static CHANNELS: Lazy<TokioMutex<HashMap<GuildId, Vec<ChannelId>>>> =
                Lazy::new(|| TokioMutex::new(HashMap::new()));

            let mut subscriptions = CHANNELS.lock().await;

            let subscribed_channels = subscriptions.entry(guild_id).or_default();
            let send = !subscribed_channels.contains(&channel_id);
            subscribed_channels.insert(0, channel_id);
            if subscribed_channels.len() > 5 {
                subscribed_channels.pop();
            }
            let full = subscribed_channels.len() == 1;

            if send {
                let channels_obj = subscribed_channels
                    .iter()
                    .map(|&ch| (ch, vec![vec![0, 99]]))
                    .collect();
                let info = if full {
                    super::custom_commands::GuildSubscriptionInfo::Full(
                        super::custom_commands::GuildSubscriptionFull {
                            guild_id,
                            typing: true,
                            activities: true,
                            threads: true,
                            channels: channels_obj,
                        },
                    )
                } else {
                    super::custom_commands::GuildSubscriptionInfo::Minimal(
                        super::custom_commands::GuildSubscriptionMinimal {
                            guild_id,
                            channels: channels_obj,
                        },
                    )
                };
                if let Err(e) = inner
                    .shard
                    .command(&super::custom_commands::GuildSubscription { d: info, op: 14 })
                    .await
                {
                    tracing::warn!(guild.id=?guild_id, channel.id=?channel_id, "Unable to send guild subscription (14): {}", e);
                }
            }
        }
    }

    // Runs on weechat runtime
    #[allow(clippy::too_many_lines)]
    pub async fn handle_events(
        mut rx: Receiver<PluginMessage>,
        conn: &ConnectionInner,
        config: Config,
        instance: Instance,
    ) {
        loop {
            let event = match rx.recv().await {
                Some(e) => e,
                None => {
                    Weechat::print("discord: error receiving message");
                    return;
                },
            };

            match event {
                PluginMessage::Connected { user } => {
                    Weechat::print(&format!("discord: ready as: {}", user.tag()));
                    tracing::info!("Ready as {}", user.tag());

                    let guilds: Vec<_> = if config.join_all() {
                        conn.cache
                            .guild_ids()
                            .expect("Cache always returns some")
                            .into_iter()
                            .map(|guild_id| {
                                (guild_id, GuildConfig::new_autoconnect_detached(guild_id))
                            })
                            .collect()
                    } else {
                        config.guilds().into_iter().collect()
                    };

                    for (guild_id, guild_config) in guilds {
                        if let Some(twilight_guild) = conn.cache.guild(guild_id) {
                            Guild::try_create(
                                twilight_guild,
                                &instance,
                                conn,
                                guild_config,
                                &config,
                            );
                        } else {
                            tracing::warn!(guild.id=%guild_id, "Guild not in cache");
                        }
                    }

                    for channel_id in config.autojoin_private() {
                        if let Some(channel) = conn.cache.private_channel(channel_id) {
                            if let Err(e) = DiscordConnection::create_private_channel(
                                conn, &config, &instance, &channel,
                            ) {
                                tracing::warn!(
                                    ?channel_id,
                                    channel.name = %channel.name(),
                                    "Unable to join private channel: {}",
                                    e
                                );
                            }
                        } else {
                            tracing::warn!("Unable to find channel: {}", channel_id);
                        }
                    }

                    for channel_id in config.watched_private() {
                        if let Some(channel) = conn.cache.private_channel(channel_id) {
                            if channel.last_message_id()
                                == conn
                                    .cache
                                    .read_state(channel_id)
                                    .map(|rs| rs.last_message_id)
                            {
                                continue;
                            }

                            if let Err(e) = DiscordConnection::create_private_channel(
                                conn, &config, &instance, &channel,
                            ) {
                                tracing::warn!(
                                    ?channel_id,
                                    channel.name = %channel.name(),
                                    "Unable to join private channel: {}",
                                    e
                                );
                            }
                        } else {
                            tracing::warn!("Unable to find channel: {}", channel_id);
                        }
                    }
                },
                PluginMessage::MessageCreate { message } => {
                    if config.watched_private().contains(&message.channel_id)
                        && !instance
                            .borrow_private_channels()
                            .contains_key(&message.channel_id)
                    {
                        let channel_id = message.channel_id;
                        if let Some(channel) = conn.cache.private_channel(channel_id) {
                            if let Err(e) = DiscordConnection::create_private_channel(
                                conn, &config, &instance, &channel,
                            ) {
                                tracing::warn!(
                                    ?channel_id,
                                    channel.name = %channel.name(),
                                    "Unable to join private channel: {}",
                                    e
                                );
                            }
                        }
                    }

                    let channel = if let Some(guild_id) = message.guild_id {
                        let channels = match instance.borrow_guilds().get(&guild_id) {
                            Some(guild) => guild.channels(),
                            None => continue,
                        };

                        match channels.get(&message.channel_id) {
                            Some(channel) => channel.clone(),
                            None => continue,
                        }
                    } else {
                        let private_channels = instance.borrow_private_channels_mut();
                        match private_channels.get(&message.channel_id) {
                            Some(channel) => channel.clone(),
                            None => continue,
                        }
                    };
                    channel.add_message(&message.into());
                },
                PluginMessage::MessageDelete { event } => {
                    if let Some(guild_id) = event.guild_id {
                        let channels = match instance.borrow_guilds().get(&guild_id) {
                            Some(guild) => guild.channels(),
                            None => continue,
                        };

                        let channel = match channels.get(&event.channel_id) {
                            Some(channel) => channel,
                            None => continue,
                        };

                        channel.remove_message(event.id);
                    } else {
                        let private_channels = instance.borrow_private_channels_mut();
                        let channel = match private_channels.get(&event.channel_id) {
                            Some(channel) => channel,
                            None => continue,
                        };

                        channel.remove_message(event.id);
                    }
                },
                PluginMessage::MessageUpdate { message } => {
                    if let Some(guild_id) = message.guild_id {
                        let channels = match instance.borrow_guilds().get(&guild_id) {
                            Some(guild) => guild.channels(),
                            None => continue,
                        };

                        let channel = match channels.get(&message.channel_id) {
                            Some(channel) => channel,
                            None => continue,
                        };

                        channel.update_message(*message);
                    } else {
                        let private_channels = instance.borrow_private_channels_mut();
                        let channel = match private_channels.get(&message.channel_id) {
                            Some(channel) => channel,
                            None => continue,
                        };

                        channel.update_message(*message);
                    }
                },
                PluginMessage::MemberChunk(member_chunk) => {
                    let channel_id = member_chunk
                        .nonce
                        .and_then(|id| id.parse().ok().map(ChannelId));
                    if !member_chunk.not_found.is_empty() {
                        tracing::warn!(
                            "Member chunk included unknown users: {:?}",
                            member_chunk.not_found
                        );
                    }
                    if let Some(channel_id) = channel_id {
                        let channels = match instance.borrow_guilds().get(&member_chunk.guild_id) {
                            Some(guild) => guild.channels(),
                            None => continue,
                        };

                        let channel = match channels.get(&channel_id) {
                            Some(channel) => channel,
                            None => continue,
                        };
                        channel.redraw(&member_chunk.not_found);
                    }
                },
                PluginMessage::TypingStart(typing) => {
                    if conn
                        .cache
                        .current_user()
                        .map(|current_user| current_user.id == typing.user_id)
                        .unwrap_or(true)
                    {
                        continue;
                    };
                    let typing_user_id = typing.user_id;
                    if let Some(name) = typing
                        .member
                        .map(|m| m.display_name().to_owned())
                        .or_else(|| conn.cache.user(typing_user_id).map(|u| u.name.clone()))
                    {
                        instance.borrow_typing_tracker_mut().add(TypingEntry {
                            channel_id: typing.channel_id,
                            guild_id: typing.guild_id,
                            user: typing.user_id,
                            user_name: name,
                            time: typing.timestamp,
                        });
                        Weechat::bar_item_update("discord_typing");
                        conn.rt
                            .spawn(async move {
                                tokio::time::sleep(Duration::from_secs(10)).await;
                            })
                            .await
                            .expect("Task is never aborted");

                        Weechat::spawn({
                            let instance = instance.clone();
                            async move {
                                instance.borrow_typing_tracker_mut().sweep();
                                Weechat::bar_item_update("discord_typing");
                            }
                        })
                        .detach();
                    }
                },
                PluginMessage::ChannelUpdate(channel_update) => {
                    if unsafe { Weechat::weechat() }.current_buffer().channel_id()
                        == Some(channel_update.0.id())
                    {
                        Weechat::bar_item_update("discord_slowmode_cooldown");
                    }
                },
                PluginMessage::ReactionAdd(reaction_add) => {
                    let reaction = reaction_add.0;
                    if let Some(guild_id) = reaction.guild_id {
                        let channels = match instance.borrow_guilds().get(&guild_id) {
                            Some(guild) => guild.channels(),
                            None => continue,
                        };

                        let channel = match channels.get(&reaction.channel_id) {
                            Some(channel) => channel,
                            None => continue,
                        };

                        channel.add_reaction(&conn.cache, &reaction);
                    } else {
                        let private_channels = instance.borrow_private_channels_mut();
                        let channel = match private_channels.get(&reaction.channel_id) {
                            Some(channel) => channel,
                            None => continue,
                        };

                        channel.add_reaction(&conn.cache, &reaction);
                    }
                },
                PluginMessage::ReactionRemove(reaction_remove) => {
                    let reaction = reaction_remove.0;
                    match reaction.guild_id {
                        Some(guild_id) => {
                            let channels = match instance.borrow_guilds().get(&guild_id) {
                                Some(guild) => guild.channels(),
                                None => continue,
                            };

                            let channel = match channels.get(&reaction.channel_id) {
                                Some(channel) => channel,
                                None => continue,
                            };

                            channel.remove_reaction(&reaction);
                        },
                        None => {
                            let private_channels = instance.borrow_private_channels_mut();
                            let channel = match private_channels.get(&reaction.channel_id) {
                                Some(channel) => channel,
                                None => continue,
                            };

                            channel.remove_reaction(&reaction);
                        },
                    }
                },
                PluginMessage::MemberListUpdate(update) => {
                    let mut member_lists = instance.borrow_member_lists_mut();
                    let member_list = member_lists.entry(update.guild_id).or_default();
                    let guild_id = update.guild_id;

                    member_list.apply_update(*update);

                    let channel_id =
                        match unsafe { Weechat::weechat() }.current_buffer().channel_id() {
                            Some(channel_id) => channel_id,
                            None => continue,
                        };

                    let channels = match instance.borrow_guilds().get(&guild_id) {
                        Some(guild) => guild.channels(),
                        None => continue,
                    };

                    let channel = match channels.get(&channel_id) {
                        Some(channel) => channel,
                        None => continue,
                    };

                    if let Some(channel_memberlist) =
                        member_list.get_list_for_channel(channel_id, &conn.cache)
                    {
                        channel.update_nicklist(channel_memberlist);
                    }
                },
            }
        }
    }

    pub fn create_private_channel(
        conn: &ConnectionInner,
        config: &Config,
        instance: &Instance,
        channel: &PrivateChannel,
    ) -> anyhow::Result<Channel> {
        let last_message_id = channel.last_message_id();
        let channel_id = channel.id;
        let channel =
            crate::buffer::channel::Channel::private(&channel, &conn, &config, &instance)?;

        if let Some(read_state) = conn.cache.read_state(channel_id) {
            if last_message_id > Some(read_state.last_message_id) {
                channel.mark_unread(read_state.mention_count.map(|mc| mc > 0).unwrap_or(false));
            }
        }

        instance
            .borrow_private_channels_mut()
            .insert(channel_id, channel.clone());

        Ok(channel)
    }

    // Runs on Tokio runtime
    async fn handle_gateway_event(event: GatewayEvent, tx: Sender<PluginMessage>) {
        match event {
            GatewayEvent::Ready(ready) => tx
                .send(PluginMessage::Connected { user: ready.user })
                .await
                .ok()
                .unwrap(),
            GatewayEvent::MessageCreate(message) => tx
                .send(PluginMessage::MessageCreate {
                    message: Box::new(message.0),
                })
                .await
                .ok()
                .expect("Receiving thread has died"),
            GatewayEvent::MessageDelete(event) => tx
                .send(PluginMessage::MessageDelete { event })
                .await
                .ok()
                .expect("Receiving thread has died"),
            GatewayEvent::MessageDeleteBulk(event) => {
                for id in event.ids {
                    tx.send(PluginMessage::MessageDelete {
                        event: twilight_model::gateway::payload::MessageDelete {
                            channel_id: event.channel_id,
                            guild_id: event.guild_id,
                            id,
                        },
                    })
                    .await
                    .ok()
                    .expect("Receiving thread has died");
                }
            },
            GatewayEvent::MessageUpdate(message) => tx
                .send(PluginMessage::MessageUpdate { message })
                .await
                .ok()
                .expect("Receiving thread has died"),
            GatewayEvent::MemberChunk(member_chunk) => tx
                .send(PluginMessage::MemberChunk(member_chunk))
                .await
                .ok()
                .expect("Receiving thread has died"),
            GatewayEvent::TypingStart(typing_start) => tx
                .send(PluginMessage::TypingStart(*typing_start))
                .await
                .ok()
                .expect("Receiving thread has died"),
            GatewayEvent::ChannelUpdate(channel_update) => tx
                .send(PluginMessage::ChannelUpdate(channel_update))
                .await
                .ok()
                .expect("Receiving thread has died"),
            GatewayEvent::ReactionAdd(reaction_add) => tx
                .send(PluginMessage::ReactionAdd(reaction_add))
                .await
                .ok()
                .expect("Receiving thread has died"),
            GatewayEvent::MemberListUpdate(update) => tx
                .send(PluginMessage::MemberListUpdate(update))
                .await
                .ok()
                .expect("Receiving thread has died"),
            GatewayEvent::ReactionRemove(reaction_remove) => tx
                .send(PluginMessage::ReactionRemove(reaction_remove))
                .await
                .ok()
                .expect("Receiving thread has died"),
            _ => {},
        }
    }
}
