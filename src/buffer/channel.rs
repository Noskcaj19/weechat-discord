use crate::{
    config::Config,
    discord::discord_connection::ConnectionInner,
    instance::Instance,
    match_map,
    nicklist::Nicklist,
    refcell::RefCell,
    twilight_utils::{
        ext::{CacheExt, ChannelExt, GuildChannelExt, MessageExt},
        DynamicChannel,
    },
    weecord_renderer::{WeecordMessage, WeecordRenderer},
};
use parsing::{Emoji, LineEdit};
use rand::{thread_rng, Rng};
use std::{borrow::Cow, rc::Rc};
use twilight_cache_inmemory::{model::CachedGuild as TwilightGuild, InMemoryCache};
use twilight_http::request::channel::reaction::RequestReactionType;
use twilight_model::{
    channel::{
        message::MessageReaction, GuildChannel as TwilightGuildChannel, Message,
        PrivateChannel as TwilightPrivateChannel, Reaction,
    },
    gateway::payload::{MemberListItem, MessageUpdate},
    guild::Permissions,
    id::{ChannelId, EmojiId, GuildId, MessageId, UserId},
    user::User,
};
use weechat::{
    buffer::{Buffer, BufferBuilder},
    Weechat,
};

struct ChannelBuffer {
    pub renderer: WeecordRenderer,
    pub nicklist: Nicklist,
}

impl ChannelBuffer {
    #[allow(clippy::too_many_arguments)]
    pub fn guild(
        name: &str,
        topic: Option<String>,
        nick: &str,
        guild_name: &str,
        id: ChannelId,
        guild_id: GuildId,
        conn: &ConnectionInner,
        config: &Config,
        instance: &Instance,
    ) -> anyhow::Result<Self> {
        let clean_guild_name = crate::utils::clean_name(&guild_name);
        let clean_channel_name = crate::utils::clean_name(&name);
        let buffer_name = format!("discord.{}.{}", clean_guild_name, clean_channel_name);

        let weechat = unsafe { Weechat::weechat() };

        if let Some(buffer) = weechat.buffer_search(crate::PLUGIN_NAME, &buffer_name) {
            if instance.search_buffer(Some(guild_id), id).is_none() {
                buffer.close();
            }
        };

        let handle = BufferBuilder::new(&buffer_name)
            .input_callback({
                let conn = conn.clone();
                let instance = instance.clone();
                move |_: &Weechat, _: &Buffer, input: Cow<str>| {
                    if let Some(channel) = instance.search_buffer(Some(guild_id), id) {
                        send_message(&channel, &conn, &input);
                    }
                    Ok(())
                }
            })
            .close_callback({
                let name = name.to_owned();
                let instance = instance.clone();
                move |_: &Weechat, _: &Buffer| {
                    tracing::trace!(buffer.id=%id, buffer.name=%name, "Buffer close");
                    instance
                        .borrow_channels_mut()
                        .remove(&id)
                        .expect("channel must be in instance")
                        .set_closed();
                    Ok(())
                }
            })
            .build()
            .map_err(|_| anyhow::anyhow!("Unable to create channel buffer"))?;

        let buffer = handle
            .upgrade()
            .map_err(|_| anyhow::anyhow!("Unable to create guild buffer"))?;

        buffer.set_localvar("nick", nick);

        if let Some(topic) = topic {
            buffer.set_title(&topic);
        }
        buffer.set_short_name(&format!("#{}", name));
        buffer.set_localvar("type", "channel");
        buffer.set_localvar("server", &clean_guild_name);
        buffer.set_localvar("channel", &clean_channel_name);
        buffer.set_localvar("guild_id", &guild_id.0.to_string());
        buffer.set_localvar("channel_id", &id.0.to_string());

        buffer.enable_hotlist();
        buffer.enable_nicklist();

        let handle = Rc::new(handle);
        Ok(Self {
            renderer: WeecordRenderer::new(conn, Rc::clone(&handle), config),
            nicklist: Nicklist::new(conn, Some(guild_id), handle),
        })
    }

    pub fn private(
        channel: &TwilightPrivateChannel,
        conn: &ConnectionInner,
        config: &Config,
        instance: &Instance,
    ) -> anyhow::Result<Self> {
        let id = channel.id;

        let short_name = Self::short_name(&channel.recipients);
        let buffer_id = Self::buffer_id(&channel.recipients);

        let weechat = unsafe { Weechat::weechat() };

        if let Some(buffer) = weechat.buffer_search(crate::PLUGIN_NAME, &buffer_id) {
            buffer.close();
        };

        let handle = BufferBuilder::new(&buffer_id)
            .input_callback({
                let conn = conn.clone();
                let instance = instance.clone();
                move |_: &Weechat, _: &Buffer, input: Cow<str>| {
                    if let Some(channel) = instance.search_buffer(None, id) {
                        send_message(&channel, &conn, &input);
                    }
                    Ok(())
                }
            })
            .close_callback({
                let short_name = short_name.to_string();
                let instance = instance.clone();
                move |_: &Weechat, _: &Buffer| {
                    tracing::trace!(buffer.id=%id, buffer.name=%short_name, "Buffer close");
                    instance
                        .borrow_private_channels_mut()
                        .remove(&id)
                        .expect("private channel must be in instance")
                        .set_closed();
                    Ok(())
                }
            })
            .build()
            .map_err(|_| anyhow::anyhow!("Unable to create channel buffer"))?;

        let buffer = handle
            .upgrade()
            .map_err(|_| anyhow::anyhow!("Unable to create guild buffer"))?;

        buffer.set_localvar("nick", &Self::nick(&conn.cache));

        let full_name = channel.name();

        buffer.set_short_name(&short_name);
        buffer.set_full_name(&full_name);
        buffer.set_title(&full_name);
        // This causes the buffer to be indented, what are the implications for not setting it?
        // buffer.set_localvar("type", "private");
        buffer.set_localvar("channel_id", &id.0.to_string());

        buffer.enable_nicklist();

        let handle = Rc::new(handle);
        Ok(Self {
            renderer: WeecordRenderer::new(&conn, Rc::clone(&handle), config),
            nicklist: Nicklist::new(conn, None, handle),
        })
    }

    fn nick(cache: &InMemoryCache) -> String {
        format!(
            "@{}",
            cache
                .current_user()
                .map(|u| u.name.clone())
                .expect("No current user?")
        )
    }

    fn short_name(recipients: &[User]) -> String {
        format!(
            "DM with {}",
            recipients
                .iter()
                .map(|u| u.name.clone())
                .collect::<Vec<_>>()
                .join(", ")
        )
    }

    fn buffer_id(recipients: &[User]) -> String {
        format!(
            "discord.dm.{}",
            &recipients
                .iter()
                .map(|u| crate::utils::clean_name(&u.name))
                .collect::<Vec<_>>()
                .join(".")
        )
    }

    pub fn close(&self) {
        if let Ok(buffer) = self.renderer.buffer_handle().upgrade() {
            buffer.close();
        }
    }

    pub fn add_bulk_msgs(&self, msgs: impl DoubleEndedIterator<Item = Message>) {
        self.renderer.add_bulk_msgs(msgs);
    }

    pub fn add_msg(&self, msg: &WeecordMessage) {
        self.renderer.add_msg(msg);
    }

    pub fn add_reaction(&self, cache: &InMemoryCache, reaction: &Reaction) {
        self.renderer.update_message(reaction.message_id, |msg| {
            // Copied from twilight
            if let Some(msg_reaction) = msg.reactions.iter_mut().find(|r| r.emoji == reaction.emoji)
            {
                if !msg_reaction.me {
                    if let Some(current_user) = cache.current_user() {
                        if current_user.id == reaction.user_id {
                            msg_reaction.me = true;
                        }
                    }
                }

                msg_reaction.count += 1;
            } else {
                let me = cache
                    .current_user()
                    .map(|user| user.id == reaction.user_id)
                    .unwrap_or_default();

                msg.reactions.push(MessageReaction {
                    count: 1,
                    emoji: reaction.emoji.clone(),
                    me,
                });
            }
        });
        self.renderer.redraw_buffer(&[]);
    }

    pub fn remove_reaction(&self, reaction: &Reaction) {
        self.renderer.update_message(reaction.message_id, |msg| {
            // TODO: Use Vec::drain_filter when it stabilizes
            if let Some((i, reaction)) = msg
                .reactions
                .iter_mut()
                .enumerate()
                .find(|(_, r)| r.emoji == reaction.emoji)
            {
                if reaction.count == 1 {
                    msg.reactions.remove(i);
                } else {
                    reaction.count -= 1;
                }
            }
        });
        self.renderer.redraw_buffer(&[]);
    }

    pub fn remove_msg(&self, id: MessageId) {
        self.renderer.remove_msg(id);
    }

    pub fn update_msg(&self, update: MessageUpdate) {
        self.renderer.apply_message_update(update);
    }

    pub fn redraw_buffer(&self, ignore_users: &[UserId]) {
        self.renderer.redraw_buffer(ignore_users);
    }
}

struct ChannelInner {
    conn: ConnectionInner,
    buffer: ChannelBuffer,
    closed: bool,
}

impl ChannelInner {
    pub fn new(conn: ConnectionInner, buffer: ChannelBuffer) -> Self {
        Self {
            conn,
            buffer,
            closed: false,
        }
    }
}

impl Drop for ChannelInner {
    fn drop(&mut self) {
        // This feels ugly, but without it, closing a buffer runs the close callback, which drops,
        // this struct, which in turn causes a segfault, as the buffer has already been explicitly
        // closed
        if self.closed {
            return;
        }

        self.buffer.close();
    }
}

#[derive(Clone)]
pub struct Channel {
    pub id: ChannelId,
    guild_id: Option<GuildId>,
    inner: Rc<RefCell<ChannelInner>>,
    config: Config,
    private: bool,
}

impl Channel {
    pub fn debug_counts(&self) -> (usize, usize) {
        (Rc::strong_count(&self.inner), Rc::weak_count(&self.inner))
    }

    pub fn guild(
        channel: &TwilightGuildChannel,
        guild: &TwilightGuild,
        conn: &ConnectionInner,
        config: &Config,
        instance: &Instance,
    ) -> anyhow::Result<Self> {
        let nick = format!(
            "@{}",
            crate::twilight_utils::current_user_nick(&guild, &conn.cache).build()
        );
        let channel_name = config
            .guilds()
            .get(&guild.id)
            .and_then(|g| g.channel_renames().get(&channel.id()).cloned())
            .unwrap_or_else(|| channel.name().to_owned());
        let channel_buffer = ChannelBuffer::guild(
            &channel_name,
            channel.topic(),
            &nick,
            &guild.name,
            channel.id(),
            guild.id,
            conn,
            config,
            instance,
        )?;
        let inner = Rc::new(RefCell::new(ChannelInner::new(
            conn.clone(),
            channel_buffer,
        )));
        Ok(Channel {
            id: channel.id(),
            guild_id: Some(guild.id),
            inner,
            config: config.clone(),
            private: false,
        })
    }

    pub fn private(
        channel: &TwilightPrivateChannel,
        conn: &ConnectionInner,
        config: &Config,
        instance: &Instance,
    ) -> anyhow::Result<Self> {
        let channel_buffer = ChannelBuffer::private(&channel, conn, config, instance)?;
        let inner = Rc::new(RefCell::new(ChannelInner::new(
            conn.clone(),
            channel_buffer,
        )));
        Ok(Channel {
            id: channel.id,
            guild_id: None,
            inner,
            config: config.clone(),
            private: true,
        })
    }

    pub fn mark_unread(&self, mention: bool) {
        self.inner
            .borrow()
            .buffer
            .add_msg(&WeecordMessage::new_notification(mention, self.private));
    }

    pub async fn load_history(&self) -> anyhow::Result<()> {
        let last_msg = self.inner.borrow().buffer.renderer.nth_oldest_message(0);
        let conn = self.inner.borrow().conn.clone();

        let result: twilight_http::Result<_> = conn
            .rt
            .spawn({
                let id = self.id;
                let msg_count = self.config.message_fetch_count() as u64;
                let conn = conn.clone();
                async move {
                    let message_fetcher = conn
                        .http
                        .channel_messages(id)
                        .limit(msg_count)
                        .expect("msg count option is limited to 100");
                    let mut messages = match last_msg {
                        Some(last_msg) => {
                            tracing::trace!("Getting history before id: {}", last_msg.id());
                            message_fetcher.before(last_msg.id()).await?
                        },
                        None => message_fetcher.await?,
                    };

                    // This is a bit of a hack because the returned messages have no guild id, even if
                    // they are from a guild channel
                    if let Some(guild_channel) = conn.cache.guild_channel(id) {
                        for msg in &mut messages {
                            msg.guild_id = guild_channel.guild_id();
                        }
                    }
                    Ok(messages)
                }
            })
            .await
            .expect("Task is never aborted");
        let messages = result?;

        let inner = self.inner.borrow();
        if let Some(read_state) = inner.conn.cache.read_state(self.id) {
            tracing::trace!(channel.id=?self.id, "Last read message id: {}", read_state.last_message_id);
            inner
                .buffer
                .renderer
                .set_last_read_id(read_state.last_message_id);
        }
        inner.buffer.add_bulk_msgs(messages.into_iter().rev());
        Ok(())
    }

    pub async fn ack(&self) -> anyhow::Result<()> {
        let conn = self.inner.borrow().conn.clone();

        let last_displayed_id = self
            .inner
            .borrow()
            .buffer
            .renderer
            .messages()
            .borrow()
            .iter()
            .filter(|msg| !matches!(msg, WeecordMessage::LocalEcho { .. }))
            .map(|msg| msg.id())
            .next();
        let last_displayed_id = match last_displayed_id {
            Some(last_id) => last_id,
            None => return Ok(()),
        };

        if let Some(last_read_message) = conn.cache.read_state(self.id).map(|rs| rs.last_message_id)
        {
            // The user must have read a message that was deleted
            if last_read_message >= last_displayed_id {
                tracing::trace!(
                    "Skipping ack, last read message {:?} >= channels' last message {:?}",
                    last_read_message,
                    last_displayed_id,
                );
                return Ok(());
            }
        }

        let result = conn
            .rt
            .spawn({
                let id = self.id;
                let http = conn.http.clone();
                let token = conn.cache.ack_token();

                async move { http.ack_message(id, last_displayed_id, token).await }
            })
            .await
            .expect("Task is never aborted");
        // This endpoint returns a new token every time, not really sure how it's supposed to be used
        // but this is a best attempt
        let token = result?;
        conn.cache.set_ack_token(&token.token);
        Ok(())
    }

    pub fn load_users(&self, instance: &Instance) -> anyhow::Result<()> {
        let conn = self.inner.borrow().conn.clone();
        if let Some(guild_id) = self.guild_id {
            let mut member_lists = instance.borrow_member_lists_mut();
            let member_list = member_lists.entry(guild_id).or_default();

            if let Some(channel_memberlist) = member_list.get_list_for_channel(self.id, &conn.cache)
            {
                self.update_nicklist(channel_memberlist);
            }
        }
        Ok(())
    }

    pub fn add_message(&self, msg: &WeecordMessage) {
        self.inner.borrow().buffer.add_msg(msg);
    }

    pub fn add_reaction(&self, cache: &InMemoryCache, reaction: &Reaction) {
        self.inner.borrow().buffer.add_reaction(cache, reaction);
    }

    pub fn remove_reaction(&self, reaction: &Reaction) {
        self.inner.borrow().buffer.remove_reaction(reaction);
    }

    pub fn remove_message(&self, msg_id: MessageId) {
        self.inner.borrow().buffer.remove_msg(msg_id);
    }

    pub fn update_message(&self, update: MessageUpdate) {
        self.inner.borrow().buffer.update_msg(update);
    }

    pub fn redraw(&self, ignore_users: &[UserId]) {
        self.inner.borrow().buffer.redraw_buffer(ignore_users);
    }

    pub fn set_closed(&self) {
        let _ = self
            .inner
            .try_borrow_mut()
            .map(|mut inner| inner.closed = true);
    }

    pub fn update_nicklist(&self, member_list: &[MemberListItem]) {
        self.inner.borrow().buffer.nicklist.update(member_list);
    }
}

#[allow(clippy::too_many_lines)]
fn send_message(channel: &Channel, conn: &ConnectionInner, input: &str) {
    let channel = channel.clone();
    let id = channel.id;
    let guild_id = channel.guild_id;
    let conn = conn.clone();
    let cache = conn.cache.clone();
    let http = conn.http.clone();
    let input = crate::twilight_utils::content::create_mentions(&cache, guild_id, &input);
    match parsing::LineEdit::parse(&input) {
        Some(LineEdit::Sub {
            line,
            old,
            new,
            options,
        }) => {
            if let Some(msg) = channel
                .inner
                .borrow()
                .buffer
                .renderer
                .get_nth_message(line - 1)
            {
                let msg = match msg {
                    WeecordMessage::Text(msg) => *msg,
                    WeecordMessage::LocalEcho { .. } => return,
                    #[cfg(feature = "images")]
                    WeecordMessage::Image { msg, .. } => *msg,
                    WeecordMessage::Notification { .. } => return,
                };

                if !msg.is_own(&cache) {
                    if let Some(has_manage) = has_manage_message_perm(&channel, &cache) {
                        if !has_manage {
                            Weechat::print(
                                "discord: you don't have permission to edit other users messages \
                                 in this channel",
                            );
                            tracing::trace!(?channel.id, "Not editing message, user does not have permission");
                        }
                    } else {
                        tracing::warn!(?channel.id, ?msg.id, "Unable to determine if user has manage messages permission, attempting to edit anyway");
                    }
                    return;
                }
                let orig = msg.content.clone();
                let old = old.to_owned();
                let new = new.to_owned();
                let options = options.map(ToString::to_string);

                conn.rt.spawn(async move {
                    let e = http.update_message(id, msg.id);
                    let future = if options.map(|o| o.contains('g')).unwrap_or_default() {
                        e.content(orig.replace(&old, &new))
                    } else {
                        e.content(orig.replacen(&old, &new, 1))
                    }
                    .expect("new content is always Some");

                    if let Err(e) = future.await {
                        tracing::error!("Unable to update message: {}", e);
                    } else {
                        tracing::trace!("Successfully updated message {}", msg.id);
                    };
                });
            } else {
                tracing::warn!("Unable to find message n {}", line);
                Weechat::print(&format!("discord: unable to locate message n = {}", line));
            };
        },
        Some(LineEdit::Delete { line }) => {
            if let Some(msg) = channel
                .inner
                .borrow()
                .buffer
                .renderer
                .get_nth_message(line - 1)
            {
                let msg = match msg {
                    WeecordMessage::Text(msg) => msg,
                    WeecordMessage::LocalEcho { .. } => return,
                    WeecordMessage::Notification { .. } => return,
                    #[cfg(feature = "images")]
                    WeecordMessage::Image { msg, .. } => msg,
                };
                if !msg.is_own(&cache) {
                    if let Some(has_manage) = has_manage_message_perm(&channel, &cache) {
                        if !has_manage {
                            Weechat::print(
                                "discord: you don't have permission to delete other users \
                                 messages in this channel",
                            );
                            tracing::trace!(?channel.id, "Not deleting message, user does not have permission");
                            return;
                        }
                    } else {
                        tracing::warn!(?channel.id, ?msg.id, "Unable to determine if user has manage messages permission, attempting to delete anyway");
                    }
                }
                // TODO: Check if user has permission to delete messages
                conn.rt.spawn(async move {
                    if let Err(e) = http.delete_message(id, msg.id).await {
                        tracing::error!("Unable to delete message: {}", e);
                    } else {
                        tracing::trace!("Successfully deleted message {}", msg.id);
                    };
                });
            } else {
                tracing::warn!("Unable to find message n {}", line);
                Weechat::print(&format!("discord: unable to locate message n = {}", line));
            };
        },
        None => {
            if let Some(reaction) = parsing::Reaction::parse(&input) {
                let add = reaction.add;
                let reaction_type = match request_from_reaction(&reaction) {
                    Some(reaction) => reaction,
                    None => return,
                };

                // TODO: Check if user can add reactions
                if let Some(msg) = channel
                    .inner
                    .borrow()
                    .buffer
                    .renderer
                    .get_nth_message(reaction.line - 1)
                {
                    let msg = match msg {
                        WeecordMessage::Text(msg) => msg,
                        WeecordMessage::LocalEcho { .. } => return,
                        #[cfg(feature = "images")]
                        WeecordMessage::Image { msg, .. } => msg,
                        WeecordMessage::Notification { .. } => return,
                    };
                    conn.rt.spawn(async move {
                        if add {
                            if let Err(e) = http.create_reaction(id, msg.id, reaction_type).await {
                                tracing::error!("Failed to add reaction: {:#?}", e);
                                Weechat::spawn_from_thread(async move {
                                    Weechat::print(&format!(
                                        "discord: an error occurred adding reaction: {}",
                                        e
                                    ));
                                });
                            }
                        } else if let Err(e) = http
                            .delete_current_user_reaction(id, msg.id, reaction_type)
                            .await
                        {
                            tracing::error!("Failed to remove reaction: {:#?}", e);
                            Weechat::spawn_from_thread(async move {
                                Weechat::print(&format!(
                                    "discord: an error occurred removing reaction: {}",
                                    e
                                ));
                            });
                        }
                    });
                }

                return;
            };

            if let Some(can_send) = cache
                .dynamic_channel(channel.id)
                .and_then(|channel| channel.can_send(&cache))
            {
                if !can_send {
                    Weechat::print(
                        "discord: you don't have permission to send messages in this channel",
                    );
                    tracing::trace!(?channel.id, "Not sending message, user does not have permission");
                    return;
                }
            } else {
                tracing::warn!(?channel.id, "Unable to determine if user has send permission, attempting to send anyway");
            }

            // Create a nonce to associate the local echo with the incoming message
            let nonce = thread_rng().gen_range(0..=i64::MAX as u64);
            conn.rt.spawn({
                let input = input.clone();
                async move {
                    match http.create_message(id).nonce(nonce).content(input) {
                        Ok(msg) => {
                            if let Err(e) = msg.await {
                                tracing::error!("Failed to send message: {:?}", e);
                                Weechat::spawn_from_thread(async move {
                                    Weechat::print(&format!(
                                        "discord: an error occurred sending message: {}",
                                        e
                                    ));
                                });
                            };
                        },
                        Err(e) => {
                            tracing::error!("Failed to create message: {:?}", e);
                            Weechat::spawn_from_thread(async move {
                                Weechat::print("discord: message content is invalid");
                            });
                        },
                    }
                }
            });
            let username = cache.current_user().unwrap().name.clone();
            channel
                .inner
                .borrow()
                .buffer
                .renderer
                .add_msg(&WeecordMessage::new_echo(username, input, nonce));
        },
    };
}

fn has_manage_message_perm(channel: &Channel, cache: &InMemoryCache) -> Option<bool> {
    match_map!(
        cache.dynamic_channel(channel.id),
        Some(DynamicChannel::Guild(channel)) => channel,
    )
    .and_then(|discord_channel| {
        discord_channel.has_permission(&cache, Permissions::MANAGE_MESSAGES)
    })
}

fn request_from_reaction(reaction: &parsing::Reaction) -> Option<RequestReactionType> {
    Some(match reaction.emoji {
        Emoji::Shortcode(name) => {
            if let Some(emoji) = discord_emoji::lookup(name) {
                RequestReactionType::Unicode {
                    name: emoji.to_owned(),
                }
            } else {
                return None;
            }
        },
        Emoji::Custom(name, id) => RequestReactionType::Custom {
            id: EmojiId(id),
            name: Some(name.to_owned()),
        },
        Emoji::Unicode(name) => RequestReactionType::Unicode {
            name: name.to_owned(),
        },
    })
}
