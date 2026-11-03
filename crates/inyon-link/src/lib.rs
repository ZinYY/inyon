//! The phone link: how a phone and a machine find each other and talk, end to end encrypted.
//! A port of packages/link (removed in 0.23) that must stay byte for byte compatible with it
//! (fixtures/link holds the vectors it wrote), so an older phone or machine on the TypeScript link
//! still talks to this one. No runtime in here: sockets, timers and HTTP belong to the caller, and the crate builds for
//! wasm32 as well as native.

pub mod channel;
pub mod code;
pub mod identity;
mod js;
pub mod noise;
pub mod pair;
pub mod pkarr;
pub mod relay;

pub use channel::{accept, connect, Accepting, Channel, ChannelError, Connecting, LinkError, PROLOGUE};
pub use code::{format_short_id, new_short_code, parse_short_code, short_code_identity, short_secret, ShortCode};
pub use identity::{is_machine_id, noise_key_of, verify, z32decode, z32encode, Identity};
pub use noise::{Initiator, KeyPair, NoiseError, Responder, Transport};
pub use pair::{pair_link, parse_pair_link, PairInfo};
pub use pkarr::{decode_published, encode_published, Published, Record, PKARR_RELAYS};
pub use relay::{auth_message, auth_message_v2, frame, machine_path, phone_path, relay_origin, relay_ws, unframe, MIN_NONCE};
