use crate::{
    config::Config,
    refcell::{RefCell, RefMut},
};
use std::{
    borrow::Cow,
    collections::HashMap,
    rc::{Rc, Weak},
};
use twilight_model::id::{ChannelId, GuildId};
use weechat::{
    config::{BooleanOptionSettings, ConfigSection, StringOption, StringOptionSettings},
    Weechat,
};

#[derive(Clone, Debug)]
pub struct GuildConfigInner {
    autoconnect: bool,
    autojoin: Vec<ChannelId>,
    watched: Vec<ChannelId>,
    channel_renames: HashMap<ChannelId, String>,
}

impl GuildConfigInner {
    pub fn new() -> Self {
        GuildConfigInner {
            autoconnect: false,
            autojoin: Vec::new(),
            watched: Vec::new(),
            channel_renames: HashMap::new(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct GuildConfig {
    inner: Rc<RefCell<GuildConfigInner>>,
    id: GuildId,
}

impl GuildConfig {
    /// Creates a guild config not attached to any weechat options
    pub fn new_autoconnect_detached(id: GuildId) -> Self {
        let mut inner = GuildConfigInner::new();
        inner.autoconnect = true;
        Self {
            inner: Rc::new(RefCell::new(inner)),
            id,
        }
    }

    pub fn new(guild_section: &mut ConfigSection, id: GuildId) -> Self {
        let inner = Rc::new(RefCell::new(GuildConfigInner::new()));

        let weak_inner = Rc::downgrade(&inner);

        let inner_clone = Weak::clone(&weak_inner);
        let autoconnect = BooleanOptionSettings::new(format!("{}.autoconnect", id.0))
            .description("Should this guild autoconnect")
            .set_change_callback(move |_, option| {
                let inner = inner_clone.upgrade().expect("Config has outlived guild");

                inner.borrow_mut().autoconnect = option.value();
            });

        guild_section
            .new_boolean_option(autoconnect)
            .expect("Unable to create autoconnect option");

        let inner_clone = Weak::clone(&weak_inner);
        let autojoin_channels = StringOptionSettings::new(format!("{}.autojoin", id.0))
            .description("The list of all channels to automatically join")
            .set_check_callback(Config::check_channels_option)
            .set_change_callback(move |_, option| {
                let inner = inner_clone.upgrade().expect("Config has outlived guild");

                let channels = Config::clean_channels_option(option);

                inner.borrow_mut().autojoin = channels;
            });
        guild_section
            .new_string_option(autojoin_channels)
            .expect("Unable to create autojoin channels option");

        let inner_clone = Weak::clone(&weak_inner);
        let watched_channels = StringOptionSettings::new(format!("{}.watched", id.0))
            .description("The list of all channels to join when unread")
            .set_check_callback(Config::check_channels_option)
            .set_change_callback(move |_, option| {
                let inner = inner_clone.upgrade().expect("Config has outlived guild");

                let channels = Config::clean_channels_option(option);

                inner.borrow_mut().watched = channels;
            });
        guild_section
            .new_string_option(watched_channels)
            .expect("Unable to create watched channels option");

        let inner_clone = Weak::clone(&weak_inner);
        let channel_renames = StringOptionSettings::new(format!("{}.channel_renames", id.0))
            .description("The mapping of channels to rename in weechat")
            .default_value("{}")
            .set_check_callback(|_: &Weechat, _: &StringOption, value: Cow<str>| {
                Self::parse_channel_id_mapping(&value).is_ok()
            })
            .set_change_callback(move |_, option| {
                let inner = inner_clone.upgrade().expect("Config has outlived guild");

                let renames = Self::parse_channel_id_mapping(&option.value()).unwrap_or_default();

                inner.borrow_mut().channel_renames = renames;
            });
        guild_section
            .new_string_option(channel_renames)
            .expect("Unable to create channel renames option");

        GuildConfig { inner, id }
    }

    // Parses the channel renames format (current a json map)
    fn parse_channel_id_mapping(value: &str) -> anyhow::Result<HashMap<ChannelId, String>> {
        Ok(serde_json::from_str::<HashMap<ChannelId, String>>(value)?)
    }

    fn serialize_channel_id_mapping(value: &HashMap<ChannelId, String>) -> anyhow::Result<String> {
        Ok(serde_json::to_string(value)?)
    }

    pub fn autoconnect(&self) -> bool {
        self.inner.borrow().autoconnect
    }

    pub fn set_autoconnect(&self, autoconnect: bool) {
        self.inner.borrow_mut().autoconnect = autoconnect;
    }

    pub fn autojoin_channels(&self) -> Vec<ChannelId> {
        self.inner.borrow().autojoin.clone()
    }

    pub fn autojoin_channels_mut(&self) -> RefMut<Vec<ChannelId>> {
        RefMut::map(self.inner.borrow_mut(), |i| &mut i.autojoin)
    }

    pub fn watched_channels(&self) -> Vec<ChannelId> {
        self.inner.borrow().watched.clone()
    }

    pub fn channel_renames(&self) -> HashMap<ChannelId, String> {
        self.inner.borrow().channel_renames.clone()
    }

    pub fn persist(&self, config: &Config) {
        let config = config.config.borrow();
        let section = config
            .search_section("server")
            .expect("Unable to get server section");

        let autojoin = section
            .search_option(&format!("{}.autojoin", self.id))
            .expect("autojoin option does not exist");
        autojoin.set(
            &self
                .autojoin_channels()
                .iter()
                .map(|c| c.0.to_string())
                .collect::<Vec<_>>()
                .join(","),
            false,
        );

        let watched = section
            .search_option(&format!("{}.watched", self.id))
            .expect("watched option does not exist");
        watched.set(
            &self
                .watched_channels()
                .iter()
                .map(|c| c.0.to_string())
                .collect::<Vec<_>>()
                .join(","),
            false,
        );

        let channel_renames = section
            .search_option(&format!("{}.channel_renames", self.id))
            .expect("channel renames option does not exist");
        channel_renames.set(
            &Self::serialize_channel_id_mapping(&self.channel_renames()).unwrap_or_default(),
            false,
        );

        let autoconnect = section
            .search_option(&format!("{}.autoconnect", self.id))
            .expect("autoconnect option does not exist");
        autoconnect.set(if self.autoconnect() { "true" } else { "false" }, false);
    }
}
