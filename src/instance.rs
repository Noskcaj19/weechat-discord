use crate::{
    buffer::{channel::Channel, guild::Guild, pins::Pins},
    discord::typing_indicator::TypingTracker,
    twilight_utils::MemberList,
};
use parking_lot::{
    lock_api::{RwLockReadGuard, RwLockWriteGuard},
    RawRwLock, RwLock,
};
use std::{collections::HashMap, rc::Rc};
use twilight_model::id::{ChannelId, GuildId};

#[derive(Clone)]
pub struct Instance {
    guilds: Rc<RwLock<HashMap<GuildId, Guild>>>,
    private_channels: Rc<RwLock<HashMap<ChannelId, Channel>>>,
    pins: Rc<RwLock<HashMap<(GuildId, ChannelId), Pins>>>,
    typing_tracker: Rc<RwLock<TypingTracker>>,
    member_lists: Rc<RwLock<HashMap<GuildId, MemberList>>>,
}

impl Instance {
    pub fn new() -> Self {
        Self {
            guilds: Rc::new(RwLock::new(HashMap::new())),
            private_channels: Rc::new(RwLock::new(HashMap::new())),
            pins: Rc::new(RwLock::new(HashMap::new())),
            typing_tracker: Rc::new(RwLock::new(TypingTracker::new())),
            member_lists: Rc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn borrow_guilds(&self) -> RwLockReadGuard<'_, RawRwLock, HashMap<GuildId, Guild>> {
        self.guilds.read()
    }

    pub fn try_borrow_guilds_mut(
        &self,
    ) -> Option<RwLockWriteGuard<'_, RawRwLock, HashMap<GuildId, Guild>>> {
        self.guilds.try_write()
    }

    pub fn borrow_guilds_mut(&self) -> RwLockWriteGuard<'_, RawRwLock, HashMap<GuildId, Guild>> {
        self.guilds.write()
    }

    pub fn borrow_private_channels(
        &self,
    ) -> RwLockReadGuard<'_, RawRwLock, HashMap<ChannelId, Channel>> {
        self.private_channels.read()
    }

    pub fn borrow_private_channels_mut(
        &self,
    ) -> RwLockWriteGuard<'_, RawRwLock, HashMap<ChannelId, Channel>> {
        self.private_channels.write()
    }

    pub fn try_borrow_private_channels_mut(
        &self,
    ) -> Option<RwLockWriteGuard<'_, RawRwLock, HashMap<ChannelId, Channel>>> {
        self.private_channels.try_write()
    }

    pub fn borrow_pins_mut(
        &self,
    ) -> RwLockWriteGuard<'_, RawRwLock, HashMap<(GuildId, ChannelId), Pins>> {
        self.pins.write()
    }

    pub fn borrow_typing_tracker_mut(&self) -> RwLockWriteGuard<'_, RawRwLock, TypingTracker> {
        self.typing_tracker.write()
    }

    pub fn search_buffer(
        &self,
        guild_id: Option<GuildId>,
        channel_id: ChannelId,
    ) -> Option<Channel> {
        if let Some(guild_id) = guild_id {
            if let Some(guild) = self.guilds.read().get(&guild_id) {
                return guild.channels().get(&channel_id).cloned();
            }
        } else {
            return self.private_channels.read().get(&channel_id).cloned();
        }

        None
    }

    pub fn borrow_member_lists(
        &self,
    ) -> RwLockReadGuard<'_, RawRwLock, HashMap<GuildId, MemberList>> {
        self.member_lists.read()
    }

    pub fn borrow_member_lists_mut(
        &self,
    ) -> RwLockWriteGuard<'_, RawRwLock, HashMap<GuildId, MemberList>> {
        self.member_lists.write()
    }
}
