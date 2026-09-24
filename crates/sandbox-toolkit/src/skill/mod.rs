//! The Skill resource API.
//!
//! A skill is discovered from a fixed `.agents/skills` directory rather than
//! registered, so [`http`] is the only call surface; the wire types live in
//! [`model`].

mod http;
pub(crate) mod model;

pub(crate) use self::http::router;
