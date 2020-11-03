use std::io;
use weechat::{buffer::BufferBuilder, Weechat};

pub struct Debug;

impl Debug {
    pub fn create_buffer() {
        if let Ok(buffer) = BufferBuilder::new("weecord.tracing").build() {
            if let Ok(buffer) = buffer.upgrade() {
                buffer.set_title("Tracing events for weecord");
                buffer.disable_hotlist();
            }
        }
    }

    pub async fn write_to_buffer(msg: Vec<u8>) {
        let message = String::from_utf8(msg).unwrap();
        let message = Weechat::execute_modifier("color_decode_ansi", "1", &message).unwrap();
        if let Some(buffer) =
            unsafe { Weechat::weechat() }.buffer_search("weecord", "weecord.tracing")
        {
            buffer.print(&message);
        }
    }
}

impl io::Write for Debug {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if !crate::SHUTTING_DOWN.triggered() {
            Weechat::spawn_from_thread(Debug::write_to_buffer(buf.to_owned()));
        }

        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
