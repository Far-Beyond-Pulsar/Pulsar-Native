use pulsar_std::env;

// Engine constants
pub const ENGINE_NAME:         &str = env!("CARGO_PKG_NAME");
pub const ENGINE_LICENSE:      &str = env!("CARGO_PKG_LICENSE");
pub const ENGINE_AUTHORS:      &str = env!("CARGO_PKG_AUTHORS");
pub const ENGINE_VERSION:      &str = env!("CARGO_PKG_VERSION");
pub const ENGINE_HOMEPAGE:     &str = env!("CARGO_PKG_HOMEPAGE");
pub const ENGINE_REPOSITORY:   &str = env!("CARGO_PKG_REPOSITORY");
pub const ENGINE_DESCRIPTION:  &str = env!("CARGO_PKG_DESCRIPTION");
pub const ENGINE_LICENSE_FILE: &str = env!("CARGO_PKG_LICENSE_FILE");

// Discord Application ID for Rich Presence
pub const DISCORD_APP_ID: &str = match option_env!("DISCORD_APP_ID") {
	Some(val) => val,
	None => "1450965386014228491",
};