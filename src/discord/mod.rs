mod discord_client;
mod event_handler;
pub mod format;

use self::discord_client::DiscordClient;
use crate::MAIN_BUFFER;
use lazy_static::lazy_static;
use serenity::client::Context;
use serenity::prelude::Mutex;
use std::{sync::Arc, thread};

pub static mut CONTEXT: std::mem::MaybeUninit<Context> = std::mem::MaybeUninit::uninit();

pub fn get_ctx() -> &'static Context {
    unsafe { return std::mem::transmute(CONTEXT.get_ref()) }
}

lazy_static! {
    pub(crate) static ref DISCORD: Arc<Mutex<Option<DiscordClient>>> = Arc::new(Mutex::new(None));
}

pub fn init(token: &str, irc_mode: bool) {
    MAIN_BUFFER.print("Connecting to Discord...");
    let (discord_client, events) = DiscordClient::start(token).unwrap();

    thread::spawn(move || {
        if let Ok(event_handler::WeecordEvent::Ready(ready)) = events.recv() {
            MAIN_BUFFER.print("Connected to Discord!");

            if !irc_mode {
                crate::buffers::create_buffers(&ready);
            }
        }
    });

    *DISCORD.lock() = Some(discord_client);
}
