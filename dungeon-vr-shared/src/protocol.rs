pub mod challenge_token;
pub mod connect_challenge_packet;
pub mod connect_init_packet;
pub mod packet;
pub mod sealed;

/// A buffer size large enough for any UDP payload carried over IPv4 or IPv6.
pub const SAFE_RECV_BUFFER_SIZE: usize = 65527;
pub const GAME_ID: u64 = 0xd54747a389d9991f;
