//! A skill is discovered from a fixed `.agents/skills` directory rather than
//! registered, so [`http`] is the only call surface.

mod http;
pub(crate) mod model;

pub(crate) use self::http::router;
