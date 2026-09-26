//! The FileSystem API: one fixed endpoint per method, with the operation chosen
//! by the `type` query parameter and every argument carried in the JSON body.
//! Workspace mode mounts it under a workspace id, direct mode under `/fs`.

mod dir;
mod file;
mod glob;
mod http;
mod meta;
pub(crate) mod model;

pub(crate) use self::http::router;
