//! Type-safe HTTP/WebDAV method constants with conversion from `axum::http::Method`.

use axum::http::Method as HttpMethod;

pub use axum::http::method::InvalidMethod;

/// Type-safe WebDAV-aware HTTP method.
///
/// Wraps both standard HTTP methods (GET, PUT, DELETE, etc.) and WebDAV
/// extension methods (PROPFIND, MKCOL, COPY, MOVE, LOCK, UNLOCK).
/// Supports conversion from `axum::http::Method` via `TryFrom`.
///
/// ```
/// use axum::http::Method as HttpMethod;
/// use sbx::webdav::Method;
///
/// let m = Method::try_from(&HttpMethod::GET).unwrap();
/// assert!(matches!(m, Method::GET));
///
/// let m = Method::try_from(&HttpMethod::from_bytes(b"PROPFIND").unwrap()).unwrap();
/// assert!(matches!(m, Method::PROPFIND));
///
/// assert!(Method::try_from(&HttpMethod::from_bytes(b"POST").unwrap()).is_err());
/// ```
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct Method(Inner);

#[derive(Clone, PartialEq, Eq, Hash)]
enum Inner {
    Head,
    Get,
    Put,
    Patch,
    Delete,
    Options,
    Propfind,
    Proppatch,
    Mkcol,
    Copy,
    Move,
    Lock,
    Unlock,
    Report,
}

impl Method {
    /// GET
    pub const GET: Self = Self(Inner::Get);

    /// HEAD
    pub const HEAD: Self = Self(Inner::Head);

    /// PUT
    pub const PUT: Self = Self(Inner::Put);

    /// PATCH
    pub const PATCH: Self = Self(Inner::Patch);

    /// DELETE
    pub const DELETE: Self = Self(Inner::Delete);

    /// OPTIONS
    pub const OPTIONS: Self = Self(Inner::Options);

    /// PROPFIND
    pub const PROPFIND: Self = Self(Inner::Propfind);

    /// PROPPATCH
    pub const PROPPATCH: Self = Self(Inner::Proppatch);

    /// MKCOL
    pub const MKCOL: Self = Self(Inner::Mkcol);

    /// COPY
    pub const COPY: Self = Self(Inner::Copy);

    /// MOVE
    pub const MOVE: Self = Self(Inner::Move);

    /// LOCK
    pub const LOCK: Self = Self(Inner::Lock);

    /// UNLOCK
    pub const UNLOCK: Self = Self(Inner::Unlock);

    /// REPORT
    pub const REPORT: Self = Self(Inner::Report);

    /// Convert from `http::Method` to `webdav::Method`.
    ///
    /// Standard HTTP methods (GET, HEAD, PUT, etc.) are matched directly.
    /// WebDAV extension methods (PROPFIND, MKCOL, etc.) are matched by
    /// string comparison.
    ///
    /// # Errors
    ///
    /// Returns an `InvalidMethod` error if the method is not recognized as a
    /// standard HTTP method or a supported WebDAV extension method.
    ///
    /// # Panics
    ///
    /// Panics if `HttpMethod::from_bytes(b"")` ever returns `Ok`, which
    /// would indicate an upstream behavior change in the `http` crate.
    /// In practice this never occurs — an empty byte string is always
    /// an invalid HTTP method.
    pub(crate) fn from_http_method(method: &HttpMethod) -> Result<Self, InvalidMethod> {
        match *method {
            HttpMethod::GET => Ok(Self::GET),
            HttpMethod::HEAD => Ok(Self::HEAD),
            HttpMethod::PUT => Ok(Self::PUT),
            HttpMethod::PATCH => Ok(Self::PATCH),
            HttpMethod::DELETE => Ok(Self::DELETE),
            HttpMethod::OPTIONS => Ok(Self::OPTIONS),
            _ => match method.as_str() {
                "PROPFIND" => Ok(Self::PROPFIND),
                "PROPPATCH" => Ok(Self::PROPPATCH),
                "MKCOL" => Ok(Self::MKCOL),
                "COPY" => Ok(Self::COPY),
                "MOVE" => Ok(Self::MOVE),
                "LOCK" => Ok(Self::LOCK),
                "UNLOCK" => Ok(Self::UNLOCK),
                "REPORT" => Ok(Self::REPORT),
                _ => Err(HttpMethod::from_bytes(b"").unwrap_err()),
            },
        }
    }

    /// Get the string representation of the method.
    #[inline]
    pub(crate) fn as_str(&self) -> &'static str {
        match self.0 {
            Inner::Head => "HEAD",
            Inner::Get => "GET",
            Inner::Put => "PUT",
            Inner::Patch => "PATCH",
            Inner::Delete => "DELETE",
            Inner::Options => "OPTIONS",
            Inner::Propfind => "PROPFIND",
            Inner::Proppatch => "PROPPATCH",
            Inner::Mkcol => "MKCOL",
            Inner::Copy => "COPY",
            Inner::Move => "MOVE",
            Inner::Lock => "LOCK",
            Inner::Unlock => "UNLOCK",
            Inner::Report => "REPORT",
        }
    }
}

impl std::fmt::Display for Method {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.as_str().fmt(f)
    }
}

impl std::convert::TryFrom<&HttpMethod> for Method {
    type Error = InvalidMethod;

    fn try_from(value: &HttpMethod) -> Result<Self, Self::Error> {
        Self::from_http_method(value)
    }
}
