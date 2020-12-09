use crate::{
    buffers, discord, on_main, on_main_blocking, utils, weechat_utils::MessageManager, Discord,
};
use lazy_static::lazy_static;
use serenity::{
    cache::CacheRwLock,
    model::{gateway::Ready, prelude::*},
    prelude::*,
};
use std::{
    collections::HashMap,
    sync::{mpsc::Sender, Arc},
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const MAX_TYPING_EVENTS: usize = 50;

#[derive(Debug, PartialEq, Eq, Ord)]
pub struct TypingEntry {
    pub channel_id: ChannelId,
    pub guild_id: Option<GuildId>,
    pub user: UserId,
    pub user_name: String,
    pub time: u64,
}

impl PartialOrd for TypingEntry {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        self.time.partial_cmp(&other.time)
    }
}

pub struct TypingTracker {
    pub entries: Vec<TypingEntry>,
}

impl TypingTracker {
    /// Remove any expired entries
    pub fn sweep(&mut self) {
        let now = SystemTime::now();
        let timestamp_now = now
            .duration_since(UNIX_EPOCH)
            .expect("Time went backwards")
            .as_secs() as u64;

        // If the entry is more than 10 seconds old, remove it
        // TODO: Use binary heap or other structure for better performance?
        self.entries.retain(|e| timestamp_now - e.time < 10)
    }
}

lazy_static! {
    pub static ref TYPING_EVENTS: Arc<Mutex<TypingTracker>> = Arc::new(Mutex::new(TypingTracker {
        entries: Vec::new(),
    }));
}

pub struct Handler {
    sender: Arc<Mutex<Sender<Ready>>>,
    watched_channels: Vec<utils::GuildOrChannel>,
}

impl Handler {
    pub fn new(weecord: &Discord, sender: Arc<Mutex<Sender<Ready>>>) -> Handler {
        let watched_channels = weecord.config.watched_channels();

        Handler {
            sender,
            watched_channels,
        }
    }
}

impl EventHandler for Handler {
    fn channel_create(&self, _ctx: Context, channel: Arc<RwLock<GuildChannel>>) {
        let channel = channel.read();
        print_guild_status_message(
            channel.guild_id,
            &format!(
                "New {} channel `{}` created",
                channel.kind.name(),
                channel.name()
            ),
        );
    }

    fn channel_delete(&self, _ctx: Context, channel: Arc<RwLock<GuildChannel>>) {
        let channel = channel.read();
        print_guild_status_message(
            channel.guild_id,
            &format!("Channel `{}` deleted", channel.name()),
        );
    }

    fn channel_pins_update(&self, _ctx: Context, pin: ChannelPinsUpdateEvent) {
        buffers::load_pin_buffer_history_for_id(pin.channel_id);
    }

    fn channel_update(&self, ctx: Context, old: Option<Channel>, new: Channel) {
        // TODO: Notify more events?
        // * Groups: user learve/join
        // * guild channel: ?
        match new {
            Channel::Category(new) => {
                // TODO: old doesn't ever seem to be available
                if let Some(old) = old.and_then(Channel::category) {
                    let new = new.read();
                    let old = old.read();

                    let guild_id = new
                        .id
                        .to_channel_cached(&ctx)
                        .and_then(Channel::guild)
                        .map(|ch| ch.read().guild_id);

                    if let Some(guild_id) = guild_id {
                        if new.name != old.name {
                            print_guild_status_message(
                                guild_id,
                                &format!("Category `{}` renamed to `{}`", old.name, new.name),
                            );
                        }
                    }
                }
            },
            Channel::Guild(new) => {
                if let Some(old) = old.and_then(Channel::guild) {
                    let new = new.read();
                    let old = old.read();

                    if new.name != old.name {
                        print_guild_status_message(
                            new.guild_id,
                            &format!("Category `{}` renamed to `{}`", old.name, new.name),
                        );
                    }
                }
            },
            _ => {},
        }
    }

    fn guild_member_update(&self, ctx: Context, old: Option<Member>, new: Member) {
        thread::spawn(move || {
            buffers::update_member_nick(&old, &new);
            if ctx.cache.read().user.id == new.user_id() {
                buffers::update_nick();
            }
        });
    }

    fn guild_members_chunk(
        &self,
        ctx: Context,
        guild_id: GuildId,
        _offline_members: HashMap<UserId, Member>,
        nonce: Option<String>,
    ) {
        on_main(move |weecord| {
            if let Some(channel_id) = nonce {
                if let Ok(channel_id) = channel_id.parse::<u64>().map(|id| ChannelId(id)) {
                    if let Some(buffer) = weecord
                        .buffer_manager
                        .get_buffer(&utils::buffer_id_for_channel(Some(guild_id), channel_id))
                    {
                        buffer.redraw_buffer(&ctx.cache);
                    }
                }
            }
        });
    }

    fn message(&self, ctx: Context, msg: Message) {
        let string_channel = utils::buffer_id_for_channel(msg.guild_id, msg.channel_id);
        let () = on_main_blocking(move |weecord| {
            if let Some(buffer) = weecord.buffer_manager.get_buffer(&string_channel) {
                print_message(&ctx.cache, &msg, &buffer);
            } else {
                match msg.channel_id.to_channel(&ctx) {
                    chan @ Ok(Channel::Private(_)) => {
                        if let Some(buffer) = weecord.buffer_manager.get_buffer(&string_channel) {
                            print_message(&ctx.cache, &msg, &buffer);
                        } else {
                            buffers::create_buffer_from_dm(
                                &ctx.cache,
                                &weecord,
                                chan.unwrap(),
                                &ctx.cache.read().user.name,
                                false,
                            );
                        }
                    },
                    chan @ Ok(Channel::Group(_)) => {
                        if let Some(buffer) = weecord.buffer_manager.get_buffer(&string_channel) {
                            print_message(&ctx.cache, &msg, &buffer);
                        } else {
                            buffers::create_buffer_from_group(
                                &ctx.cache,
                                &weecord,
                                chan.unwrap(),
                                &ctx.cache.read().user.name,
                            );
                        }
                    },
                    Ok(Channel::Guild(channel)) => {
                        // Check that the channel is on the watch list
                        let channel = channel.read();

                        for watched in &self.watched_channels {
                            use utils::GuildOrChannel::*;
                            let add = match watched {
                                Channel(_, channel_id) => *channel_id == channel.id,
                                Guild(guild_id) => *guild_id == channel.guild_id,
                            };
                            if add {
                                let guild = match channel.guild_id.to_guild_cached(&ctx.cache) {
                                    Some(guild) => guild,
                                    None => return,
                                };

                                let current_user = ctx.cache.read().user.clone();
                                let guild = guild.read();

                                // TODO: Colors?
                                let nick = if let Ok(current_member) =
                                    guild.id.member(&ctx, current_user.id)
                                {
                                    format!("@{}", current_member.display_name())
                                } else {
                                    format!("@{}", current_user.name)
                                };

                                buffers::create_guild_buffer(guild.id, &guild.name);
                                // TODO: Muting
                                buffers::create_buffer_from_channel(
                                    &ctx.cache,
                                    &guild.name,
                                    &channel,
                                    &nick,
                                    false,
                                );
                                break;
                            }
                        }
                    },
                    _ => {},
                }
            }
        });
    }

    fn message_delete(&self, ctx: Context, channel_id: ChannelId, deleted_message_id: MessageId) {
        delete_message(&ctx, channel_id, deleted_message_id)
    }

    fn message_delete_bulk(
        &self,
        ctx: Context,
        channel_id: ChannelId,
        deleted_messages_ids: Vec<MessageId>,
    ) {
        for message_id in deleted_messages_ids {
            delete_message(&ctx, channel_id, message_id)
        }
    }

    fn message_update(
        &self,
        ctx: Context,
        _old_if_available: Option<Message>,
        _new: Option<Message>,
        event: MessageUpdateEvent,
    ) {
        let (guild_id, channel_id, message_id) = match ctx.cache.read().channel(&event.channel_id) {
            Some(Channel::Guild(channel)) => {
                let channel = channel.read();
                (Some(channel.guild_id), channel.id, event.id)
            },
            Some(Channel::Group(channel)) => (None, channel.read().channel_id, event.id),
            Some(Channel::Private(channel)) => (None, channel.read().id, event.id),
            _ => return,
        };

        let buffer_name = utils::buffer_id_for_channel(guild_id, channel_id);

        thread::spawn(move || {
            on_main(move |weecord| {
                let ctx = match crate::discord::get_ctx() {
                    Some(ctx) => ctx,
                    _ => return,
                };
                let msg = match channel_id
                    .messages(ctx, |retriever| retriever.limit(1).around(message_id))
                    .ok()
                    .and_then(|mut msgs| msgs.pop())
                {
                    Some(msgs) => msgs,
                    None => return,
                };

                if let Some(buffer) = weecord.buffer_manager.get_buffer(&buffer_name) {
                    buffer.replace_message(&ctx.cache, &message_id, &msg);
                }
            });
        });
    }

    fn reaction_add(&self, ctx: Context, reaction: Reaction) {
        reaction_update(ctx, reaction)
    }

    fn reaction_remove(&self, ctx: Context, reaction: Reaction) {
        reaction_update(ctx, reaction)
    }

    fn ready(&self, ctx: Context, ready: Ready) {
        // Cache seems not to have all fields properly populated

        ctx.shard
            .chunk_guilds(ready.guilds.iter().map(GuildStatus::id), None, None);
        {
            let mut ctx_lock = ctx.cache.write();
            for (&id, channel) in &ready.private_channels {
                if let Some(pc) = channel.clone().private() {
                    ctx_lock.private_channels.insert(id, pc);
                }
            }
            for guild in &ready.guilds {
                if let GuildStatus::OnlineGuild(guild) = guild {
                    for (id, pres) in guild.presences.clone() {
                        ctx_lock.presences.insert(id, pres);
                    }

                    // TODO: Why are channels not populated by serenity?
                    for (id, chan) in guild.channels.clone() {
                        ctx_lock.channels.insert(id, chan);
                    }
                }
            }
        }
        if let Some(presence) = ctx.cache.read().presences.get(&ready.user.id) {
            *crate::command::LAST_STATUS.lock() = presence.status;
        }

        unsafe {
            crate::discord::CONTEXT = Some(ctx);
        }
        let _ = self.sender.lock().send(ready);
    }

    fn typing_start(&self, ctx: Context, event: TypingStartEvent) {
        // TODO: Do we want to fetch the user if it isn't cached? (check performance)
        let current_user_id = ctx.cache.read().user.id;

        if let Some(user) = event.user_id.to_user_cached(&ctx.cache) {
            let user = user.read();
            if user.id == current_user_id {
                return;
            }
            // TODO: Resolve guild nick names
            let mut typing_events = TYPING_EVENTS.lock();
            typing_events.entries.push(TypingEntry {
                channel_id: event.channel_id,
                guild_id: event.guild_id,
                user: event.user_id,
                user_name: user.name.clone(),
                time: event.timestamp,
            });

            typing_events.sweep();
            if typing_events.entries.len() > MAX_TYPING_EVENTS {
                typing_events.entries.pop();
            }

            crate::on_main(|weechat| {
                weechat.update_bar_item("discord_typing");
            });

            thread::Builder::new()
                .name("Typing indicator updater".into())
                .spawn(|| {
                    // Wait a few seconds, then sweep the list and update the bar item
                    thread::sleep(Duration::from_secs(10));

                    let mut typing_events = TYPING_EVENTS.lock();
                    typing_events.sweep();
                    crate::on_main(|weechat| {
                        weechat.update_bar_item("discord_typing");
                    });
                })
                .expect("Unable to name thread");
        }
    }

    fn user_update(&self, _ctx: Context, _old: CurrentUser, _new: CurrentUser) {
        thread::spawn(|| {
            // TODO: Update nicklist (and/or just rework all nick stuff)
            buffers::update_nick();
        });
    }
}

fn delete_message(ctx: &Context, channel_id: ChannelId, deleted_message_id: MessageId) {
    if let Some(channel) = ctx.cache.read().channels.get(&channel_id) {
        let guild_id = channel.read().guild_id;
        let buffer_name = utils::buffer_id_for_channel(Some(guild_id), channel_id);

        on_main(move |weecord| {
            if let Some(buffer) = weecord.buffer_manager.get_buffer(&buffer_name) {
                let ctx = match discord::get_ctx() {
                    Some(ctx) => ctx,
                    _ => return,
                };

                buffer.delete_message(&ctx.cache, &deleted_message_id);
            }
        });
    }
}

fn reaction_update(ctx: Context, reaction: Reaction) {
    let guild_id = match ctx.cache.read().channel(&reaction.channel_id) {
        Some(Channel::Guild(channel)) => {
            let channel = channel.read();
            Some(channel.guild_id)
        },
        _ => return,
    };
    let buffer_name = utils::buffer_id_for_channel(guild_id, reaction.channel_id);
    thread::spawn(move || {
        on_main(move |weecord| {
            if let Some(buffer) = weecord.buffer_manager.get_buffer(&buffer_name) {
                let ctx = match crate::discord::get_ctx() {
                    Some(ctx) => ctx,
                    _ => return,
                };
                let msg = match reaction.channel_id
                    .messages(ctx, |retriever| retriever.limit(1).around(reaction.message_id))
                    .ok()
                    .and_then(|mut msgs| msgs.pop())
                    {
                        Some(msgs) => msgs,
                        None => return,
                    };
                buffer.replace_message(&ctx.cache, &reaction.message_id, &msg);
            }
        });
    });
}


fn print_message(cache: &CacheRwLock, msg: &Message, buffer: &MessageManager) {
    let muted = utils::buffer_is_muted(&buffer);
    let notify = !msg.is_own(cache) && !muted;
    buffer.add_message(cache, &msg, notify);
}

fn print_guild_status_message(guild_id: GuildId, msg: &str) {
    let buffer_id = utils::buffer_id_for_guild(guild_id);

    let msg = msg.to_owned();
    on_main(move |weechat| {
        if let Some(buffer) = weechat.buffer_search("weecord", &buffer_id) {
            let prefix = weechat.get_prefix("network").to_owned();
            buffer.print(&(prefix + "\t" + msg.as_ref()));
        }
    })
}
