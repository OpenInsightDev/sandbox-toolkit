//! The FileSystem API: one fixed endpoint per method, with the operation chosen
//! by the `type` query parameter and every argument carried in the JSON body.
//! Workspace mode mounts it under a workspace id, direct mode under `/fs`.

mod file;
mod http;
mod meta;
pub(crate) mod model;
mod path;

pub(crate) use self::http::router;
