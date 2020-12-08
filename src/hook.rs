use crate::{
    buffers::load_pin_buffer_history,
    command::Args,
    discord, on_main, plugin_print, utils,
    utils::{BufferExt, ChannelExt},
};
use crossbeam_channel::unbounded;
use lazy_static::lazy_static;
use serenity::{client::bridge::gateway, model::prelude::*, prelude::*};
use serenity::model::channel::ReactionType;
use std::{
    iter::FromIterator,
    sync::Arc,
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use weechat::{Buffer, CompletionPosition, ConfigOption, ReturnCode, Weechat};

lazy_static! {
    static ref LAST_TYPING_TIMESTAMP: Arc<Mutex<u64>> = Arc::new(Mutex::new(0));
}

pub struct HookHandles {
    _buffer_switch_handle: weechat::SignalHook<()>,
    _buffer_typing_handle: weechat::SignalHook<()>,
    _command_handles: Vec<weechat::CommandHook<()>>,
    _query_handle: weechat::CommandRunHook<()>,
    _nick_handle: weechat::CommandRunHook<()>,
    _join_handle: weechat::CommandRunHook<()>,
    _guild_completion_handle: weechat::CompletionHook<()>,
    _channel_completion_handle: weechat::CompletionHook<()>,
    _dm_completion_handle: weechat::CompletionHook<()>,
    _nick_completion_handle: weechat::CompletionHook<()>,
    _role_completion_handle: weechat::CompletionHook<()>,
}

pub fn init(weechat: &Weechat) -> HookHandles {
    let _command_handles = crate::command::init(weechat);

    let _buffer_switch_handle = weechat.hook_signal(
        "buffer_switch",
        |_, _, value| handle_buffer_switch(value),
        None,
    );

    let _buffer_typing_handle = weechat.hook_signal(
        "input_text_changed",
        |_, weechat, value| handle_buffer_typing(weechat, value),
        None,
    );

    let _query_handle = weechat.hook_command_run(
        "/query",
        |_, ref buffer, ref command| {
            if buffer.guild_id().is_none() {
                return ReturnCode::Error;
            };

            handle_query(&Args::from_cmd(
                &command.replace("/query ", "/discord query "),
            ))
        },
        None,
    );

    let _nick_handle = weechat.hook_command_run(
        "/nick",
        |_, ref buffer, ref command| handle_nick(buffer, command),
        None,
    );

    let _join_handle = weechat.hook_command_run(
        "/join",
        |_, ref buffer, ref command| handle_join(buffer, command),
        None,
    );

    let _guild_completion_handle = weechat.hook_completion(
        "weecord_guild_completion",
        "Completion for discord guilds",
        |_, ref buffer, _, completions| handle_guild_completion(buffer, completions),
        None,
    );

    let _channel_completion_handle = weechat.hook_completion(
        "weecord_channel_completion",
        "Completion for discord channels",
        |_, ref buffer, _, completions| handle_channel_completion(buffer, completions),
        None,
    );

    let _dm_completion_handle = weechat.hook_completion(
        "weecord_dm_completion",
        "Completion for Discord private channels",
        |_, ref buffer, _, completions| handle_dm_completion(buffer, completions),
        None,
    );

    let _nick_completion_handle = weechat.hook_completion(
        "nicks",
        "Completion for users in a buffer",
        |_, ref buffer, _, completions| handle_nick_completion(buffer, completions),
        None,
    );

    let _role_completion_handle = weechat.hook_completion(
        "weecord_role",
        "Completion for Discord channel roles",
        |_, ref buffer, _, completions| handle_role_completion(buffer, completions),
        None,
    );

    HookHandles {
        _buffer_switch_handle,
        _buffer_typing_handle,
        _command_handles,
        _query_handle,
        _nick_handle,
        _join_handle,
        _guild_completion_handle,
        _channel_completion_handle,
        _dm_completion_handle,
        _nick_completion_handle,
        _role_completion_handle,
    }
}

pub fn buffer_input(buffer: Buffer, text: &str) {
    let text = if text.is_empty() {
        return;
    } else {
        text
    };
    let channel = buffer.channel_id();
    let guild = buffer.guild_id();

    if let Some(channel) = channel {
        let ctx = match crate::discord::get_ctx() {
            Some(ctx) => ctx,
            _ => return,
        };

        if let Some(edit) = parsing::parse_line_edit(text) {
            let weechat = buffer.get_weechat();
            match edit {
                parsing::LineEdit::Delete { line } => {
                    if let Err(e) = crate::utils::get_users_nth_message(&ctx, channel, line)
                        .map(|msg| channel.delete_message(&ctx.http, msg.id))
                    {
                        buffer.print(&format!(
                            "{}\tAn error occurred deleting a message: {}",
                            weechat.get_prefix("network"),
                            e
                        ));
                    }
                },
                parsing::LineEdit::Sub {
                    line,
                    old,
                    new,
                    options,
                } => {
                    // TODO: Clean this up, (try block)?
                    if let Err(e) =
                        crate::utils::get_users_nth_message(&ctx, channel, line).map(|mut msg| {
                            let orig = msg.content.clone();
                            msg.edit(ctx, |e| {
                                if options.map(|o| o.contains('g')).unwrap_or_default() {
                                    e.content(orig.replace(old, new))
                                } else {
                                    e.content(orig.replacen(old, new, 1))
                                }
                            })
                        })
                    {
                        buffer.print(&format!(
                            "{}\tAn error occurred editing a message: {}",
                            weechat.get_prefix("network"),
                            e
                        ));
                    }
                },
            }
            return;
        }
        if let Some(reaction) = parsing::parse_reaction(text) {
            if let Ok(msgs) =
                channel.messages(ctx, |retriever| retriever.limit(reaction.line as u64))
            {
                for (i, msg) in msgs.iter().enumerate() {
                    if (i + 1) == reaction.line {
                        if reaction.add {
                            let _ =
                                msg.react(ctx, ReactionType::Unicode(reaction.unicode.to_string()));
                        } else {
                            let _ = channel.delete_reaction(
                                &ctx,
                                msg.id,
                                None,
                                ReactionType::Unicode(reaction.unicode.to_string()),
                            );
                        }
                    }
                }
            }
            return;
        }
        let text = utils::create_mentions(&ctx.cache, guild, text);
        let text = utils::expand_guild_emojis(&ctx.cache, guild, &text);
        if let Err(e) = channel.say(ctx, text) {
            let weechat = buffer.get_weechat();
            buffer.print(&format!(
                "{}\tUnable to send message to {}: {:#?}",
                weechat.get_prefix("network"),
                channel.0,
                e
            ));
        }
    }
}

fn handle_buffer_switch(data: weechat::SignalHookValue) -> ReturnCode {
    if let weechat::SignalHookValue::Pointer(buffer_ptr) = data {
        let buffer = unsafe { crate::utils::buffer_from_ptr(buffer_ptr) };
        let weechat = buffer.get_weechat();
        let weecord = crate::upgrade_plugin(&weechat);
        let buffer = match weecord
            .buffer_manager
            .get_buffer(buffer.get_name().as_ref())
        {
            Some(buffer) => buffer,
            None => return ReturnCode::Ok,
        };

        // Wait until messages have been loaded to acknowledge them
        let (tx, rx) = unbounded();
        if !buffer.history_loaded() {
            let pinned_channel_id = utils::pins_for_channel(&buffer);

            if pinned_channel_id.is_some() {
                load_pin_buffer_history(&buffer);
                return ReturnCode::Ok;
            }

            let fetch_count = weecord.config.message_fetch_count.value();

            crate::buffers::load_history(&buffer, tx, fetch_count);
        }

        if !buffer.nicks_loaded() {
            crate::buffers::load_nicks(&buffer);
        }

        let channel_id = buffer.channel_id();

        thread::spawn(move || {
            if rx.recv().is_err() {
                return;
            }
            let ctx = match discord::get_ctx() {
                Some(s) => s,
                None => return,
            };
            if let Some(channel) = channel_id.and_then(|id| id.to_channel_cached(&ctx)) {
                if let Some(guild_channel) = channel.clone().guild() {
                    let guild_id = guild_channel.read().guild_id;
                    use std::collections::{HashMap, HashSet};
                    lazy_static! {
                        static ref CHANNELS: Arc<Mutex<HashMap<GuildId, HashSet<ChannelId>>>> =
                            Arc::new(Mutex::new(HashMap::new()));
                    }

                    let mut channels = CHANNELS.lock();
                    let send = if let Some(guild_channels) = channels.get_mut(&guild_id) {
                        guild_channels.insert(channel.id())
                    } else {
                        channels
                            .insert(guild_id, HashSet::from_iter(vec![channel.id()].into_iter()));
                        true
                    };
                    if send {
                        let channels = channels.get(&guild_id).unwrap();
                        let channels_obj: HashMap<String, Vec<Vec<_>>> = HashMap::from_iter(
                            channels
                                .iter()
                                .map(|ch| (format!("{}", ch.0), vec![vec![0, 99]])),
                        );

                        let msg = json::object! {
                            "op" => 14,
                            "d" => json::object! {
                                "guild_id" => format!("{}", guild_id.0),
                                "typing" => true,
                                "activities" => true,
                                "channels" => channels_obj,
                            }
                        };
                        ctx.shard
                            .websocket_message(gateway::Message::Text(msg.to_string()));
                    }
                }

                if let Some(rs) = ctx.cache.read().read_state.get(&channel.id()) {
                    if let Some(last_message_id) = channel.last_message() {
                        if rs.last_message_id != last_message_id {
                            let _ = channel.id().ack_message(&ctx, last_message_id);
                        }
                    }
                }
            }
        });
    }
    ReturnCode::Ok
}

fn handle_buffer_typing(weechat: &Weechat, data: weechat::SignalHookValue) -> ReturnCode {
    if let weechat::SignalHookValue::Pointer(buffer_ptr) = data {
        let buffer = unsafe { crate::utils::buffer_from_ptr(buffer_ptr) };
        if let Some(channel_id) = buffer.channel_id() {
            if crate::upgrade_plugin(weechat)
                .config
                .send_typing_events
                .value()
            {
                if buffer.input().starts_with('/') {
                    return ReturnCode::Ok;
                }
                // TODO: Wait for user to type for 3 seconds
                let now = SystemTime::now();
                let timestamp_now = now
                    .duration_since(UNIX_EPOCH)
                    .expect("Time went backwards")
                    .as_secs() as u64;

                if *LAST_TYPING_TIMESTAMP.lock() + 9 < timestamp_now {
                    *LAST_TYPING_TIMESTAMP.lock() = timestamp_now;

                    std::thread::spawn(move || {
                        let ctx = match discord::get_ctx() {
                            Some(s) => s,
                            None => return,
                        };
                        let _ = channel_id.broadcast_typing(&ctx.http);
                    });
                }
            }
        }
    }
    ReturnCode::Ok
}

fn handle_channel_completion(buffer: &Buffer, completion: weechat::Completion) -> ReturnCode {
    // Get the previous argument with should be the guild name
    // TODO: Generalize this?
    let input = buffer.input();
    let x = input.split(' ').collect::<Vec<_>>();
    let input = if x.len() < 2 {
        None
    } else {
        Some(x[x.len() - 2].to_owned())
    };

    let input = match input {
        Some(i) => i,
        None => return ReturnCode::Ok,
    };

    // Match mangled name to the real name
    let ctx = match discord::get_ctx() {
        Some(s) => s,
        None => return ReturnCode::Ok,
    };

    for guild in ctx.cache.read().guilds.values() {
        let guild = guild.read();
        if parsing::weechat_arg_strip(&guild.name).to_lowercase() == input.to_lowercase() {
            for channel in guild.channels.values() {
                let channel = channel.read();
                // Skip non text channels
                use serenity::model::channel::ChannelType::*;
                match channel.kind {
                    Text | Private | Group | News => {},
                    _ => continue,
                }
                let permissions = guild.user_permissions_in(channel.id, ctx.cache.read().user.id);
                if !permissions.read_message_history() || !permissions.read_messages() {
                    continue;
                }
                completion.add(&parsing::weechat_arg_strip(&channel.name))
            }
            return ReturnCode::Ok;
        }
    }
    ReturnCode::Ok
}

fn handle_guild_completion(_buffer: &Buffer, completion: weechat::Completion) -> ReturnCode {
    let ctx = match discord::get_ctx() {
        Some(s) => s,
        None => return ReturnCode::Ok,
    };
    for guild in ctx.cache.read().guilds.values() {
        let name = parsing::weechat_arg_strip(&guild.read().name);
        completion.add(&name);
    }
    ReturnCode::Ok
}

fn handle_dm_completion(_buffer: &Buffer, completion: weechat::Completion) -> ReturnCode {
    let ctx = match discord::get_ctx() {
        Some(s) => s,
        None => return ReturnCode::Ok,
    };
    for dm in ctx.cache.read().private_channels.values() {
        completion.add(&dm.read().recipient.read().name);
    }
    ReturnCode::Ok
}

fn handle_nick_completion(buffer: &Buffer, completion: weechat::Completion) -> ReturnCode {
    let ctx = match discord::get_ctx() {
        Some(s) => s,
        None => return ReturnCode::Ok,
    };

    let channel_id = buffer.channel_id();

    if let Some(Channel::Guild(channel)) = channel_id.and_then(|c| c.to_channel(ctx).ok()) {
        let channel = channel.read();

        if let Ok(members) = channel.members(&ctx.cache) {
            for member in members {
                completion.add_with_options(
                    &format!("@{}", member.distinct()),
                    false,
                    CompletionPosition::Sorted,
                );
            }
        }
    }

    ReturnCode::Ok
}

fn handle_role_completion(buffer: &Buffer, completion: weechat::Completion) -> ReturnCode {
    let ctx = match discord::get_ctx() {
        Some(s) => s,
        None => return ReturnCode::Ok,
    };

    let guild = buffer.guild_id();

    if let Some(guild) = guild {
        if let Some(guild) = guild.to_guild_cached(&ctx.cache) {
            let roles = &guild.read().roles;
            for role in roles.values() {
                completion.add(&format!("@{}", role.name));
            }
        }
    }

    ReturnCode::Ok
}

// TODO: Make this faster
// TODO: Handle command options
pub fn handle_query(args: &Args) -> ReturnCode {
    let mut owned_args = args.clone();

    let mut noswitch = false;
    if let Some(&arg) = owned_args.args.front() {
        if arg == "-noswitch" {
            noswitch = true;
            owned_args.args.pop_front();
        }
    }
    if let Some(&arg) = owned_args.args.front() {
        if arg == "-server" {
            plugin_print("The server option is not yet supported");
            return ReturnCode::Error;
        }
    }

    if owned_args.args.is_empty() {
        plugin_print("query requires a username");
        return ReturnCode::Error;
    }

    let target = match owned_args.args.pop_front() {
        Some(target) => target.to_owned(),
        None => return ReturnCode::Ok,
    };

    thread::spawn(move || {
        let ctx = match crate::discord::get_ctx() {
            Some(ctx) => ctx,
            _ => return,
        };
        let current_user = &ctx.cache.read().user;

        let mut found_members: Vec<User> = Vec::new();
        for private_channel in ctx.cache.read().private_channels.values() {
            if private_channel
                .read()
                .name()
                .to_lowercase()
                .contains(&target.to_lowercase())
            {
                found_members.push(private_channel.read().recipient.read().clone())
            }
        }

        if found_members.is_empty() {
            let guilds = current_user.guilds(ctx).expect("Unable to fetch guilds");
            for guild in &guilds {
                if let Some(guild) = guild.id.to_guild_cached(ctx) {
                    let guild = guild.read().clone();
                    for m in guild.members_containing(&target.to_lowercase(), false, true) {
                        found_members.push(m.user.read().clone());
                    }
                }
            }
        }
        found_members.dedup_by_key(|mem| mem.id);

        let current_user_name = current_user.name.clone();

        if let Some(target) = found_members.get(0) {
            if let Ok(chan) = target.create_dm_channel(ctx) {
                on_main(move |weecord| {
                    let ctx = match crate::discord::get_ctx() {
                        Some(ctx) => ctx,
                        _ => return,
                    };
                    crate::buffers::create_buffer_from_dm(
                        &ctx.cache,
                        &weecord,
                        Channel::Private(Arc::new(RwLock::new(chan))),
                        &current_user_name,
                        !noswitch,
                    );
                });
                return;
            }
        }

        plugin_print(&format!("Could not find user {:?}", target));
    });
    ReturnCode::OkEat
}

// TODO: Handle command options
fn handle_nick(buffer: &Buffer, command: &str) -> ReturnCode {
    let guild = if let Some(id) = buffer.guild_id() {
        id
    } else {
        return ReturnCode::Ok;
    };

    let guilds;
    let mut substr;
    {
        let ctx = match crate::discord::get_ctx() {
            Some(ctx) => ctx,
            _ => return ReturnCode::Error,
        };
        substr = command["/nick".len()..].trim().to_owned();
        let mut split = substr.split(' ');
        let all = split.next() == Some("-all");
        if all {
            substr = substr["-all".len()..].trim().to_owned();
        }
        guilds = if all {
            let current_user = &ctx.cache.read().user;

            // TODO: Error handling
            current_user
                .guilds(ctx)
                .unwrap_or_default()
                .iter()
                .map(|g| g.id)
                .collect()
        } else {
            vec![guild]
        };
    }

    thread::spawn(move || {
        {
            let ctx = match crate::discord::get_ctx() {
                Some(ctx) => ctx,
                _ => return,
            };
            let should_sleep = guilds.len() > 1;
            for guild in guilds {
                let new_nick = if substr.is_empty() {
                    None
                } else {
                    Some(substr.as_str())
                };
                let _ = guild.edit_nickname(ctx, new_nick);
                // Make it less spammy
                if should_sleep {
                    thread::sleep(Duration::from_secs(1));
                }
            }
        }
    });
    ReturnCode::OkEat
}

fn handle_join(buffer: &Buffer, command: &str) -> ReturnCode {
    let verbose = buffer.guild_id().is_some();

    crate::command::join(
        &buffer.get_weechat(),
        &crate::command::Args::from_cmd(&format!("/discord {}", &command[1..])),
        verbose,
    )
}
