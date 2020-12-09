use crate::utils::BufferExt;
use serenity::{
    cache::CacheRwLock,
    model::{
        channel::Message,
        id::{MessageId, UserId},
    },
};
use std::{cell::RefCell, ops::Deref, sync::Arc};
use weechat::Buffer;

/// MessageRenderer wraps a weechat buffer and facilitates editing the buffer and drawing the
/// messages
pub struct MessageManager {
    buffer: Buffer,
    messages: Arc<RefCell<Vec<Message>>>,
}

impl MessageManager {
    /// Create a new MessageManager from a buffer
    pub fn new(buffer: Buffer) -> MessageManager {
        MessageManager {
            buffer,
            messages: Arc::new(RefCell::new(Vec::new())),
        }
    }

    /// Format and print message to the buffer
    fn print_msg(&self, cache: &CacheRwLock, msg: &Message, notify: bool) -> Vec<UserId> {
        let weechat = self.buffer.get_weechat();
        let maybe_guild = self.buffer.guild_id();
        let (prefix, content, unknown_users) =
            formatting_utils::render_msg(cache, &weechat, msg, maybe_guild);
        self.buffer.print_tags_dated(
            msg.timestamp.timestamp(),
            &formatting_utils::msg_tags(cache, msg, notify).join(","),
            &format!("{}\t{}", prefix, content),
        );
        unknown_users
    }

    /// Clear the buffer and reprint all messages
    pub fn redraw_buffer(&self, cache: &CacheRwLock) {
        self.buffer.clear();
        for message in self.messages.borrow().iter() {
            self.print_msg(cache, &message, false);
        }
    }

    /// Removes all content from the buffer
    pub fn clear(&self) {
        self.messages.borrow_mut().clear();
        self.buffer.clear();
    }

    /// Add a message to the end of a buffer (chronologically)
    pub fn add_message(&self, cache: &CacheRwLock, msg: &Message, notify: bool) -> Vec<UserId> {
        let unknown_users = self.print_msg(cache, msg, notify);
        self.messages.borrow_mut().push(msg.clone());
        unknown_users
    }

    // Overwrite a previously printed message, has no effect if the message does not exist
    pub fn replace_message(
        &self,
        cache: &CacheRwLock,
        id: &MessageId,
        msg: &Message,
    ) -> Vec<UserId> {
        if let Some(old_msg) = self
            .messages
            .borrow_mut()
            .iter_mut()
            .find(|it| &it.id == id)
        {
            *old_msg = msg.clone();
        }
        // Using hdata to edit the line might be more efficient Would still use redrawing as a fall
        // back in the event that the edit has a different amount of lines
        self.redraw_buffer(cache);
        let (_, _, unknown_users) = formatting_utils::render_msg(
            cache,
            &self.buffer.get_weechat(),
            msg,
            self.buffer.guild_id(),
        );
        unknown_users
    }

    /// Delete a previously printed message, has no effect if the message does not exist
    pub fn delete_message(&self, cache: &CacheRwLock, id: &MessageId) -> Vec<UserId> {
        let index = self.messages.borrow().iter().position(|it| &it.id == id);
        let mut unknown_users = Vec::new();
        if let Some(index) = index {
            let msg = self.messages.borrow_mut().remove(index);
            unknown_users = formatting_utils::render_msg(
                cache,
                &self.buffer.get_weechat(),
                &msg,
                self.buffer.guild_id(),
            )
            .2;
        };
        // Performance, see comment above
        self.redraw_buffer(cache);
        unknown_users
    }
}

impl Deref for MessageManager {
    type Target = Buffer;

    fn deref(&self) -> &Self::Target {
        &self.buffer
    }
}

mod formatting_utils {
    use crate::{
        discord::formatting,
        utils::{colorize_string, format_nick_color},
    };
    use serenity::{
        cache::CacheRwLock,
        model::{
            channel::{Channel, Message},
            id::{GuildId, UserId},
        },
    };
    use std::str::FromStr;
    use weechat::{ConfigOption, Weechat};

    pub fn msg_tags(cache: &CacheRwLock, msg: &Message, notify: bool) -> Vec<String> {
        let is_private = if let Some(channel) = msg.channel(cache) {
            if let Channel::Private(_) = channel {
                true
            } else {
                false
            }
        } else {
            false
        };

        let self_mentioned = msg.mentions_user_id(cache.read().user.id);

        let mut tags = Vec::new();
        if notify {
            if self_mentioned {
                tags.push("notify_highlight");
            } else if is_private {
                tags.push("notify_private");
            } else {
                tags.push("notify_message");
            };
        } else {
            tags.push("notify_none");
        }

        tags.into_iter().map(ToString::to_string).collect()
    }

    pub fn render_msg(
        cache: &CacheRwLock,
        weechat: &Weechat,
        msg: &Message,
        guild: Option<GuildId>,
    ) -> (String, String, Vec<UserId>) {
        let opts = serenity::utils::ContentSafeOptions::new()
            .clean_here(false)
            .clean_everyone(false)
            .clean_user(false);

        let mut msg_content = serenity::utils::content_safe(&cache, &msg.content, &opts);
        msg_content = crate::utils::clean_emojis(&msg_content);
        let unknown_users = clean_users(cache, &mut msg_content, true, guild);

        if msg.edited_timestamp.is_some() {
            let edited_text = weechat.color("8").into_owned()
                + " (edited)"
                + &weechat.color("reset").into_owned();
            msg_content.push_str(&edited_text);
        }

        for attachement in &msg.attachments {
            if !msg_content.is_empty() {
                msg_content.push('\n');
            }
            msg_content.push_str(&attachement.proxy_url);
        }

        for embed in &msg.embeds {
            if !msg_content.is_empty() {
                msg_content.push('\n');
            }
            if let Some(ref author) = embed.author {
                msg_content.push('▎');
                msg_content.push_str(&format!(
                    "{}{}{}",
                    weechat.color("bold"),
                    author.name,
                    weechat.color("reset"),
                ));
                if let Some(url) = &author.url {
                    msg_content.push_str(&format!(" ({})", url));
                }
                msg_content.push('\n');
            }
            if let Some(ref title) = embed.title {
                msg_content.push_str(
                    &title
                        .lines()
                        .fold(String::new(), |acc, x| format!("{}▎{}\n", acc, x)),
                );
                msg_content.push('\n');
            }
            if let Some(ref description) = embed.description {
                msg_content.push_str(
                    &description
                        .lines()
                        .fold(String::new(), |acc, x| format!("{}▎{}\n", acc, x)),
                );
                msg_content.push('\n');
            }
            for field in &embed.fields {
                msg_content.push_str(&field.name);
                msg_content.push_str(
                    &field
                        .value
                        .lines()
                        .fold(String::new(), |acc, x| format!("{}▎{}\n", acc, x)),
                );
                msg_content.push('\n');
            }
            if let Some(ref footer) = embed.footer {
                msg_content.push_str(
                    &footer
                        .text
                        .lines()
                        .fold(String::new(), |acc, x| format!("{}▎{}\n", acc, x)),
                );
                msg_content.push('\n');
            }
        }

        if msg.reactions.len() > 0 {
            msg_content.push('\n');
        }

        for reaction in &msg.reactions {
            msg_content.push_str(
                &format!("[{} {}] ", reaction.reaction_type, reaction.count).as_str()
            );
        }

        if msg.reactions.len() > 0 {
            msg_content.push('\n');
        }

        let mut prefix = String::new();

        if let Some(nick_prefix) = weechat.get_string_option("weechat.look.nick_prefix") {
            if let Some(color) = weechat.get_string_option("weechat.color.chat_nick_prefix") {
                prefix.push_str(&colorize_string(
                    weechat,
                    &color.value(),
                    &nick_prefix.value(),
                ))
            }
        }

        let author = format_nick_color(weechat, &author_display_name(cache, &msg, guild));
        prefix.push_str(&author);

        if let Some(nick_suffix) = weechat.get_string_option("weechat.look.nick_suffix") {
            if let Some(color) = weechat.get_string_option("weechat.color.chat_nick_suffix") {
                prefix.push_str(&colorize_string(
                    weechat,
                    &color.value(),
                    &nick_suffix.value(),
                ))
            }
        }

        use serenity::model::channel::MessageType::*;
        if let Regular = msg.kind {
            (
                prefix,
                formatting::discord_to_weechat(weechat, &msg_content),
                unknown_users,
            )
        } else {
            let (prefix, body) = match msg.kind {
                GroupRecipientAddition | MemberJoin => {
                    ("join", format!("{} joined the group.", author))
                },
                GroupRecipientRemoval => ("quit", format!("{} left the group.", author)),
                GroupNameUpdate => (
                    "network",
                    format!("{} changed the channel name: {}.", author, msg.content),
                ),
                GroupCallCreation => ("network", format!("{} started a call.", author)),
                GroupIconUpdate => ("network", format!("{} changed the channel icon.", author)),
                PinsAdd => (
                    "network",
                    format!("{} pinned a message to this channel", author),
                ),
                NitroBoost => (
                    "network",
                    format!("{} boosted this channel with nitro", author),
                ),
                NitroTier1 => (
                    "network",
                    "This channel has achieved nitro level 1".to_string(),
                ),
                NitroTier2 => (
                    "network",
                    "This channel has achieved nitro level 2".to_string(),
                ),
                NitroTier3 => (
                    "network",
                    "This channel has achieved nitro level 3".to_string(),
                ),
                Regular | __Nonexhaustive => unreachable!(),
            };
            (weechat.get_prefix(prefix).into_owned(), body, unknown_users)
        }
    }

    pub fn author_display_name(
        cache: &CacheRwLock,
        msg: &Message,
        guild: Option<GuildId>,
    ) -> String {
        let display_name = guild.and_then(|id| {
            cache
                .read()
                .member(id, msg.author.id)
                .map(|member| member.display_name().to_string())
        });
        display_name.unwrap_or_else(|| msg.author.name.to_owned())
    }

    /// Convert raw mentions into human readable form, returning all ids that were not converted
    /// Extracted from serenity and modified
    fn clean_users(
        cache: &CacheRwLock,
        s: &mut String,
        show_discriminator: bool,
        guild: Option<GuildId>,
    ) -> Vec<UserId> {
        let mut unknown_users = Vec::new();
        let mut progress = 0;

        while let Some(mut mention_start) = s[progress..].find("<@") {
            mention_start += progress;

            if let Some(mut mention_end) = s[mention_start..].find('>') {
                mention_end += mention_start;
                mention_start += "<@".len();

                let has_exclamation = if s[mention_start..]
                    .as_bytes()
                    .get(0)
                    .map_or(false, |c| *c == b'!')
                {
                    mention_start += "!".len();

                    true
                } else {
                    false
                };

                if let Ok(id) = UserId::from_str(&s[mention_start..mention_end]) {
                    let replacement = if let Some(guild) = guild {
                        if let Some(guild) = cache.read().guild(&guild) {
                            if let Some(member) = guild.read().members.get(&id) {
                                if show_discriminator {
                                    Some(format!("@{}", member.distinct()))
                                } else {
                                    Some(format!("@{}", member.display_name()))
                                }
                            } else {
                                unknown_users.push(id);
                                None
                            }
                        } else {
                            unknown_users.push(id);
                            None
                        }
                    } else {
                        let user = cache.read().users.get(&id).cloned();

                        if let Some(user) = user {
                            let user = user.read();

                            if show_discriminator {
                                Some(format!("@{}#{:04}", user.name, user.discriminator))
                            } else {
                                Some(format!("@{}", user.name))
                            }
                        } else {
                            unknown_users.push(id);
                            None
                        }
                    };

                    let code_start = if has_exclamation { "<@!" } else { "<@" };
                    let to_replace = format!("{}{}>", code_start, &s[mention_start..mention_end]);

                    if let Some(replacement) = replacement {
                        *s = s.replace(&to_replace, &replacement);
                    } else {
                        progress = mention_end;
                    }
                } else {
                    let id = &s[mention_start..mention_end].to_string();

                    if !id.is_empty() && id.as_bytes().iter().all(u8::is_ascii_digit) {
                        let code_start = if has_exclamation { "<@!" } else { "<@" };
                        let to_replace = format!("{}{}>", code_start, id);

                        *s = s.replace(&to_replace, &"@invalid-user");
                    } else {
                        progress = mention_end;
                    }
                }
            } else {
                break;
            }
        }
        unknown_users
    }
}
