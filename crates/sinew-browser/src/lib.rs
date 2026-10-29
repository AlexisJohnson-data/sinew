mod cdp;
pub mod input;
pub mod observe;
pub mod recording;
mod session;
pub mod snapshot;

pub use observe::{ConsoleEntry, NetworkEntry, TabLog};
pub use recording::{gif_to_base64, RecordingHandle};
pub use session::{BrowserSession, BrowserSessions, Tab, SETTLE_MAX, VIEWPORT};
